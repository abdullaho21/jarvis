mod engine;
mod inference;
mod matching;
mod worker;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Deserialize)]
struct WhisperFiles {
    encoder: String,
    decoder: String,
    tokens: String,
}
#[derive(Deserialize)]
struct NativeConfig {
    vosk_library: String,
    model_path: String,
    whisper: Option<WhisperFiles>,
    denoiser: Option<String>,
    threads: i32,
    candidates: Vec<(String, String)>,
    wake_forms: Vec<String>,
    greetings: Vec<String>,
    prefixes: Vec<String>,
    suffixes: Vec<String>,
}
#[derive(Deserialize)]
struct Payload {
    config: Value,
    mode: String,
    grammar: engine::Grammar,
    native: NativeConfig,
}
fn emit(event: Value) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, &event)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}
fn read_line(reader: &mut impl BufRead, limit: usize) -> Result<Option<Vec<u8>>> {
    let mut bytes = vec![];
    (&mut *reader)
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() > limit {
        return Err("Worker input exceeds its size limit".into());
    }
    Ok((!bytes.is_empty()).then_some(bytes))
}
fn read_pcm(path: &str) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = vec![];
    file.take(320001).read_to_end(&mut bytes)?;
    if bytes.len() < 6400 || bytes.len() > 320000 || bytes.len() % 2 != 0 {
        return Err("Fixture must be 0.2–10 seconds of s16le PCM".into());
    }
    Ok(bytes)
}
fn metrics() -> Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let mib = |name| {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 1024.0
    };
    json!({"rss_mib":mib("VmRSS:"), "peak_rss_mib":mib("VmHWM:")})
}
fn cpu_seconds() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time);
    }
    time.tv_sec as f64 + time.tv_nsec as f64 / 1e9
}
fn benchmark(payload: Payload, reader: &mut impl BufRead, engine_mode: bool) -> Result<()> {
    let start = std::time::Instant::now();
    let transcriber = if engine_mode {
        None
    } else {
        Some(inference::Transcriber::new(&payload.native)?)
    };
    let mut engine = if engine_mode {
        Some(engine::Engine::new(
            &payload.native,
            &payload.grammar,
            &payload.config,
        )?)
    } else {
        None
    };
    emit(
        json!({"event":"ready","load_seconds":start.elapsed().as_secs_f64(),"memory":metrics(),"whisper":"rust-tract-0.23.9","denoiser":"rust-tract-0.23.9"}),
    )?;
    while let Some(line) = read_line(reader, 16384)? {
        let request: Value = serde_json::from_slice(&line)?;
        let pcm = read_pcm(request["path"].as_str().ok_or("Fixture path required")?)?;
        let start = std::time::Instant::now();
        let cpu = cpu_seconds();
        let mut events = vec![];
        let mut text = String::new();
        if let Some(transcriber) = &transcriber {
            text = transcriber.transcribe(&pcm)?;
        }
        if let Some(engine) = &mut engine {
            engine.begin(request["session"].as_str().unwrap_or("command") == "command");
            let mut padded = pcm.clone();
            padded.resize((pcm.len() + 64000).div_ceil(960) * 960, 0);
            for frame in padded.as_chunks::<960>().0 {
                if let Some(result) = engine.feed(frame, worker::rms(frame) < 0.005)? {
                    let wake = result.kind == "wake";
                    events.push(serde_json::to_value(&result)?);
                    if wake {
                        if let Some(command) = engine.after_wake() {
                            events.push(serde_json::to_value(command)?);
                        }
                    }
                    break;
                }
            }
        }
        let seconds = start.elapsed().as_secs_f64();
        let decode_cpu = cpu_seconds() - cpu;
        let settle = request["settle_seconds"].as_f64().unwrap_or(0.0);
        if !settle.is_finite() || !(0.0..=2.0).contains(&settle) {
            return Err("Invalid settle duration".into());
        }
        let cpu = cpu_seconds();
        std::thread::sleep(std::time::Duration::from_secs_f64(settle));
        let settle_cpu = cpu_seconds() - cpu;
        emit(
            json!({"event":"result","text":text,"events":events,"seconds":seconds,"decode_cpu_seconds":decode_cpu,"settle_cpu_seconds":settle_cpu,"memory":metrics()}),
        )?;
    }
    Ok(())
}
fn policy(reader: &mut impl BufRead) -> Result<()> {
    while let Some(line) = read_line(reader, 1048576)? {
        let request: Value = serde_json::from_slice(&line)?;
        let strings = |key: &str| {
            request[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let candidates: Vec<(String, String)> =
            serde_json::from_value(request["candidates"].clone())?;
        let matcher = matching::Matcher {
            phrases: Default::default(),
            apps: Default::default(),
            verbs: vec![],
            prefixes: strings("prefixes"),
            suffixes: strings("suffixes"),
            candidates,
        };
        emit(json!({"match":matcher.closest(request["heard"].as_str().ok_or("heard required")?)}))?;
    }
    Ok(())
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|s| !["--transcribe", "--engine", "--match", "--version"].contains(&s.as_str()))
        || args.len() > 1
    {
        return Err(
            "Use --transcribe, --engine, --match, --version, or no arguments for microphone worker"
                .into(),
        );
    }
    if args.first().is_some_and(|a| a == "--version") {
        println!("jarvis-voice 0.1.0; Rust Whisper/GTCRN tract 0.23.9; native Vosk");
        return Ok(());
    }
    worker::bind_parent()?;
    let mut reader = BufReader::new(std::io::stdin());
    if args.first().is_some_and(|a| a == "--match") {
        return policy(&mut reader);
    }
    let payload: Payload = serde_json::from_slice(
        &read_line(&mut reader, 1048576)?.ok_or("Worker configuration required")?,
    )?;
    if !(1..=32).contains(&payload.native.threads) {
        return Err("Invalid inference thread count".into());
    }
    if args
        .first()
        .is_some_and(|a| a == "--transcribe" || a == "--engine")
    {
        return benchmark(payload, &mut reader, args[0] == "--engine");
    }
    let controls = reader.buffer().to_vec();
    drop(reader);
    worker::run(payload, controls)
}
fn main() {
    if let Err(error) = run() {
        let _ = emit(json!({"event":"error","message":error.to_string()}));
        std::process::exit(1);
    }
}

use jarvis_gtcrn::Denoiser;
use std::{
    fs,
    io::{BufRead, Read, Write},
    path::PathBuf,
    time::Instant,
};

fn memory() -> (f64, f64) {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let mib = |key| {
        status
            .lines()
            .find(|s| s.starts_with(key))
            .and_then(|s| s.split_whitespace().nth(1))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 1024.0
    };
    (mib("VmRSS:"), mib("VmHWM:"))
}
fn process(
    denoiser: &Denoiser,
    input: &str,
    output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(output);
    if output.exists() {
        return Err("Output exists; choose a new output path".into());
    }
    let mut bytes = vec![];
    fs::File::open(input)?
        .take(320001)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 320000 || bytes.len() % 2 != 0 {
        return Err("Input must contain at most ten seconds of complete s16le samples".into());
    }
    let samples: Vec<f32> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
        .collect();
    let start = Instant::now();
    let enhanced = denoiser.process(&samples)?;
    let elapsed = start.elapsed();
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output)?;
    for sample in &enhanced {
        file.write_all(&sample.to_le_bytes())?;
    }
    let (rss, peak) = memory();
    println!("{{\"event\":\"result\",\"input_samples\":{},\"output_samples\":{},\"seconds\":{},\"rss_mib\":{},\"peak_rss_mib\":{}}}",samples.len(),enhanced.len(),elapsed.as_secs_f64(),rss,peak);
    std::io::stdout().flush()?;
    Ok(())
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 4 && args[0] == "--spectra" {
        let mut bytes = vec![];
        fs::File::open(&args[2])?
            .take(1_300_001)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 1300000 || !bytes.len().is_multiple_of(4) {
            return Err("Invalid spectral input length".into());
        }
        let data: Vec<_> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let output = Denoiser::load(&args[1])?.spectra(&data)?;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&args[3])?;
        for value in output {
            file.write_all(&value.to_le_bytes())?;
        }
        return Ok(());
    }
    let batch = args.len() == 2 && args[0] == "--batch";
    if !batch && args.len() != 3 {
        return Err("Usage: jarvis-denoise MODEL.onnx INPUT.s16le OUTPUT.f32le; or --batch MODEL.onnx with input/output paths separated by a tab per stdin line".into());
    }
    let start = Instant::now();
    let denoiser = Denoiser::load(&args[if batch { 1 } else { 0 }])?;
    let load = start.elapsed();
    let (rss, peak) = memory();
    println!(
        "{{\"event\":\"ready\",\"load_seconds\":{},\"rss_mib\":{},\"peak_rss_mib\":{}}}",
        load.as_secs_f64(),
        rss,
        peak
    );
    std::io::stdout().flush()?;
    if batch {
        let mut reader = std::io::stdin().lock();
        loop {
            let mut bytes = vec![];
            (&mut reader).take(16385).read_until(b'\n', &mut bytes)?;
            if bytes.is_empty() {
                break;
            }
            if bytes.len() > 16384 {
                return Err("Batch request exceeds 16 KiB".into());
            }
            let line = std::str::from_utf8(&bytes)?.trim_end_matches('\n');
            let (input, output) = line
                .split_once('\t')
                .ok_or("Expected input path, tab, output path")?;
            process(&denoiser, input, output)?;
        }
    } else {
        process(&denoiser, &args[1], &args[2])?;
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error:?}");
        std::process::exit(1);
    }
}

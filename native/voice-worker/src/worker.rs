use crate::{
    emit,
    engine::{Engine, Recognition},
    Payload, Result,
};
use serde_json::json;
use std::collections::VecDeque;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
pub fn bind_parent() -> Result<()> {
    let before = unsafe { libc::getppid() };
    let expected: i32 = std::env::var("MURMUR_OWNER_PID")
        .ok()
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(before);
    if before <= 1 || before != expected {
        return Err("Voice worker owner exited".into());
    }
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if unsafe { libc::getppid() } != before {
        return Err("Voice worker owner changed".into());
    }
    // Handler only writes an atomic; the service retains its three-second kill bound.
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
    }
    Ok(())
}
fn now() -> f64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t);
    }
    t.tv_sec as f64 + t.tv_nsec as f64 / 1e9
}
pub fn rms(pcm: &[u8]) -> f64 {
    let sum: i64 = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| {
            let v = i16::from_le_bytes([s[0], s[1]]) as i64;
            v * v
        })
        .sum();
    (sum as f64 / (pcm.len() / 2).max(1) as f64).sqrt() / 32768.0
}
struct NoiseFloor {
    minimum: f64,
    adaptive: bool,
    floor: Option<f64>,
}
impl NoiseFloor {
    fn gate(&self) -> f64 {
        if self.adaptive {
            self.floor
                .map_or(self.minimum, |f| self.minimum.max(0.08_f64.min(f * 3.0)))
        } else {
            self.minimum
        }
    }
    fn update(&mut self, energy: f64) {
        self.floor = Some(self.floor.map_or(energy, |f| {
            f + (energy - f) * if energy < f { 0.05 } else { 0.0015 }
        }));
    }
}
struct Capture(Child);
impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let start = std::time::Instant::now();
        while start.elapsed().as_secs_f64() < 2.0 {
            if !matches!(self.0.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn ready(engine: Option<&Engine>, listening: bool, deadline: Option<f64>) -> Result<()> {
    emit(
        json!({"event":"ready","listening":listening,"deadline":deadline,"unsupported":engine.map(|e| &e.unsupported).cloned().unwrap_or_default(),"engine":engine.map(|e| e.name.as_str()),"worker":"rust"}),
    )
}
fn heard(engine: &mut Option<Engine>) -> Result<()> {
    if let Some(text) = engine.as_mut().and_then(|e| e.unmatched.take()) {
        emit(json!({"event":"heard","text":text.chars().take(120).collect::<String>()}))?;
    }
    Ok(())
}
fn command(result: &Recognition, captured_at: f64) -> Result<()> {
    let mut message = serde_json::to_value(result)?;
    message["event"] = json!("command");
    message["captured_at"] = json!(captured_at);
    emit(message)
}
pub fn run(payload: Payload, mut controls: Vec<u8>) -> Result<()> {
    let Payload {
        config,
        mode,
        grammar,
        native,
    } = payload;
    if config["privacy"]["onboarded"].as_bool() != Some(true) {
        return Err("Complete microphone consent in Settings first".into());
    }
    if !["wake_word", "once", "test"].contains(&mode.as_str()) {
        return Err("Invalid worker mode".into());
    }
    let timeout = config["activation"]["command_timeout"]
        .as_f64()
        .filter(|v| v.is_finite() && (2.0..=15.0).contains(v))
        .ok_or("Invalid command timeout")?;
    let mut engine = if mode == "test" {
        None
    } else {
        emit(json!({"event":"loading","message":"Loading local voice model…"}))?;
        Some(Engine::new(&native, &grammar, &config)?)
    };
    drop(native);
    drop(grammar);
    crate::inference::trim();
    if STOP.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut capture = Command::new("pw-record");
    capture.args([
        "--raw",
        "--rate",
        "16000",
        "--channels",
        "1",
        "--format",
        "s16",
        "--latency",
        "30ms",
    ]);
    let device = config["audio"]["device"].as_str().unwrap_or("");
    if !device.is_empty() {
        capture.args(["--target", device]);
    }
    capture
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let owner = std::process::id() as i32;
    // SAFETY: only async-signal-safe libc calls run between fork and exec.
    unsafe {
        capture.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != owner {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    let mut capture = Capture(capture.spawn()?);
    let mut audio = capture.0.stdout.take().ok_or("Missing capture stdout")?;
    let mut stderr = capture.0.stderr.take().ok_or("Missing capture stderr")?;
    let mut stderr_open = true;
    let mut errors = VecDeque::with_capacity(2048);
    let mut control_open = mode == "wake_word";
    let mut listening = mode == "once";
    let mut deadline = if ["test", "once"].contains(&mode.as_str()) {
        Some(now() + if mode == "test" { 15.0 } else { timeout })
    } else {
        None
    };
    if let Some(e) = &mut engine {
        e.begin(listening);
    }
    ready(engine.as_ref(), listening, deadline)?;
    let minimum = config["audio"]["vad_rms"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.0001..=0.5).contains(v))
        .ok_or("Invalid VAD level")?;
    let mut noise = NoiseFloor {
        minimum,
        adaptive: config["audio"]["adaptive_gate"].as_bool().unwrap_or(true),
        floor: None,
    };
    let mut next_level = 0.0;
    let mut cooldown = 0.0;
    let mut settling = false;
    let mut quiet_frames = 0;
    let mut last_frame = now();
    let mut pending = [0_u8; 960];
    let mut filled = 0;
    while !STOP.load(Ordering::Relaxed) {
        let time = now();
        if time - last_frame >= 3.0 {
            return Err("No microphone frames for three seconds. Check the input device and PipeWire, then Resume.".into());
        }
        if deadline.is_some_and(|d| time >= d) {
            let stamp = deadline.unwrap();
            let late = if listening {
                if let Some(e) = &mut engine {
                    e.flush()?
                } else {
                    None
                }
            } else {
                None
            };
            last_frame = now();
            if let Some(result) = late {
                command(&result, stamp)?;
                if mode == "once" {
                    return Ok(());
                }
                listening = false;
                deadline = None;
                settling = false;
                engine.as_mut().unwrap().begin(false);
                cooldown = now() + 1.5;
                continue;
            }
            heard(&mut engine)?;
            if ["test", "once"].contains(&mode.as_str()) {
                emit(json!({"event":"timeout"}))?;
                return Ok(());
            }
            listening = false;
            deadline = None;
            settling = false;
            quiet_frames = 0;
            engine.as_mut().unwrap().begin(false);
            emit(json!({"event":"timeout"}))?;
        }
        let next = deadline.map_or(last_frame + 3.0, |d| d.min(last_frame + 3.0));
        let mut polls = [
            libc::pollfd {
                fd: if control_open { 0 } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: audio.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if stderr_open { stderr.as_raw_fd() } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let wait = if controls.contains(&b'\n') {
            0
        } else {
            ((next - now()).max(0.0) * 1000.0)
                .ceil()
                .min(i32::MAX as f64) as i32
        };
        if unsafe { libc::poll(polls.as_mut_ptr(), 3, wait) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(std::io::Error::last_os_error().into());
        }
        if polls[2].revents != 0 {
            let mut bytes = [0_u8; 512];
            let n = stderr.read(&mut bytes)?;
            if n == 0 {
                stderr_open = false;
            }
            for &b in &bytes[..n] {
                if errors.len() == 2048 {
                    errors.pop_front();
                }
                errors.push_back(b);
            }
        }
        // Control wins over audio, including a continuously readable pipe.
        if control_open && polls[0].revents != 0 {
            let mut bytes = [0_u8; 4096];
            let n = unsafe { libc::read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(std::io::Error::last_os_error().into());
            }
            if n == 0 {
                control_open = false;
            } else {
                controls.extend_from_slice(&bytes[..n as usize]);
            }
        }
        while let Some(end) = controls.iter().position(|&b| b == b'\n') {
            let line = controls.drain(..=end).collect::<Vec<_>>();
            listening = line == b"listen\n";
            deadline = if listening {
                Some(now() + timeout)
            } else {
                None
            };
            settling = false;
            quiet_frames = 0;
            cooldown = 0.0;
            engine
                .as_mut()
                .ok_or("Controls require a recognition engine")?
                .begin(listening);
            ready(engine.as_ref(), listening, deadline)?;
        }
        if controls.len() > 4096 {
            return Err("Voice control line exceeds 4096 bytes".into());
        }
        if polls[1].revents == 0 {
            continue;
        }
        let n = audio.read(&mut pending[filled..])?;
        if n == 0 {
            return Err(format!(
                "Microphone stream ended. Check PipeWire and the selected input. {}",
                String::from_utf8_lossy(&errors.into_iter().collect::<Vec<_>>())
            )
            .into());
        }
        filled += n;
        if filled < 960 {
            continue;
        }
        filled = 0;
        last_frame = now();
        if mode == "test" {
            if now() >= next_level {
                let level = rms(&pending);
                noise.update(level);
                emit(json!({"event":"level","level":level,"gate":noise.gate()}))?;
                next_level = now() + 0.2;
            }
            continue;
        }
        engine.as_mut().unwrap().maintain()?;
        if now() < cooldown {
            continue;
        }
        let energy = rms(&pending);
        let gate = noise.gate();
        noise.update(energy);
        let quiet = energy < gate;
        if settling {
            quiet_frames = if quiet { quiet_frames + 1 } else { 0 };
            if quiet_frames >= 7 {
                settling = false;
                engine.as_mut().unwrap().begin(true);
            }
            continue;
        }
        let mut captured_at = now();
        if listening && deadline.is_some_and(|d| captured_at >= d) {
            continue;
        }
        let result = engine.as_mut().unwrap().feed(&pending, quiet)?;
        last_frame = now();
        let Some(mut result) = result else {
            heard(&mut engine)?;
            continue;
        };
        if result.kind == "wake" {
            listening = true;
            deadline = Some(now() + timeout);
            settling = true;
            quiet_frames = 0;
            emit(json!({"event":"wake","deadline":deadline}))?;
            let follow = engine.as_mut().unwrap().after_wake();
            if let Some(command) = follow {
                result = command;
                settling = false;
                captured_at = now();
            } else {
                heard(&mut engine)?;
                continue;
            }
        } else {
            heard(&mut engine)?;
        }
        command(&result, captured_at)?;
        if mode == "once" {
            return Ok(());
        }
        listening = false;
        deadline = None;
        engine.as_mut().unwrap().begin(false);
        cooldown = now() + 1.5;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn energy_and_adaptation() {
        assert_eq!(rms(&[0; 960]), 0.0);
        assert_eq!(rms(&[0, 128]), 1.0);
        let mut n = NoiseFloor {
            minimum: 0.005,
            adaptive: true,
            floor: None,
        };
        n.update(0.01);
        assert!((n.gate() - 0.03).abs() < 1e-12);
        n.update(0.0);
        assert!((n.gate() - 0.0285).abs() < 1e-12);
    }
}

use crate::{desktop, err, util, Result};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::{mpsc, watch},
    time::{timeout, Duration},
};

pub fn color(cfg: &Value, monitor: &Value) -> Result<&'static str> {
    let mode = cfg["color_mode"].as_str().unwrap_or("auto");
    let hdr = ["hdr", "hdredid"].contains(
        &monitor["colorManagementPreset"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .as_str(),
    );
    if mode == "sdr" && hdr {
        return err("This display is in HDR mode. Choose Automatic or Preserve HDR, or disable display HDR before SDR capture. Direct monitor capture does not tone map HDR to SDR.");
    }
    if !["auto", "hdr", "sdr"].contains(&mode) {
        return err("Capture color mode must be auto, hdr or sdr.");
    }
    Ok(if mode == "hdr" || (mode == "auto" && hdr) {
        "hdr"
    } else {
        "sdr"
    })
}
fn gpu_args(color: &str) -> Vec<String> {
    [
        "-k",
        if color == "hdr" { "hevc_hdr" } else { "h264" },
        "-cr",
        "limited",
        "-fallback-cpu-encoding",
        "no",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
pub fn replay_command(cfg: &Value, ipc: &Path, folder: &Path, color: &str) -> Vec<String> {
    let r = &cfg["recording"];
    let replay = &cfg["replay"];
    let mut argv = vec![
        "gpu-screen-recorder".into(),
        "-w".into(),
        r["monitor"].as_str().unwrap().into(),
        "-f".into(),
        r["fps"].to_string(),
        "-r".into(),
        replay["seconds"].to_string(),
        "-replay-storage".into(),
        "ram".into(),
        "-c".into(),
        "mp4".into(),
        "-o".into(),
        folder.to_string_lossy().into(),
        "-ipc".into(),
        ipc.to_string_lossy().into(),
        "-v".into(),
        "no".into(),
    ];
    argv.extend(gpu_args(color));
    let mut sources = vec![];
    if replay["game_audio"] == true {
        sources.push("default_output");
    }
    if replay["microphone"] == true {
        sources.push("default_input");
    }
    if !sources.is_empty() {
        argv.extend(["-a".into(), sources.join("|")]);
    }
    argv
}
pub fn orphaned() -> Vec<u32> {
    let mut result = vec![];
    let Ok(entries) = fs::read_dir("/proc") else {
        return result;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse() else {
            continue;
        };
        if !entry
            .metadata()
            .is_ok_and(|m| m.uid() == unsafe { libc::getuid() })
        {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let argv: Vec<_> = bytes
            .split(|c| *c == 0)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        if argv.iter().take(6).any(|s| {
            Path::new(s)
                .file_name()
                .is_some_and(|n| n == "wf-recorder" || n == "gpu-screen-recorder")
        }) && argv.iter().any(|s| {
            Path::new(s)
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".Murmur_"))
        }) {
            result.push(pid);
        }
    }
    result
}
pub async fn verify(path: &Path, color: &str) -> Result<()> {
    if !fs::metadata(path).is_ok_and(|m| m.len() > 0) {
        return err("The recorder produced no video data.");
    }
    let data:Value=serde_json::from_str(&util::query(&["ffprobe".into(),"-v".into(),"error".into(),"-show_entries".into(),"stream=codec_type,width,height,pix_fmt,color_space,color_transfer,color_primaries,color_range".into(),"-of".into(),"json".into(),path.to_string_lossy().into()],10).await?)?;
    let video: Vec<_> = data["streams"]
        .as_array()
        .ok_or("The recorded file could not be validated.")?
        .iter()
        .filter(|s| {
            s["codec_type"] == "video"
                && s["width"].as_u64().unwrap_or(0) > 0
                && s["height"].as_u64().unwrap_or(0) > 0
        })
        .collect();
    if video.is_empty() {
        return err("The output contains no valid video stream.");
    }
    if color == "hdr"
        && !video.iter().any(|s| {
            ["yuv420p10le", "p010le", "p010be", "yuv420p10be"]
                .contains(&s["pix_fmt"].as_str().unwrap_or(""))
                && s["color_transfer"] == "smpte2084"
                && s["color_primaries"] == "bt2020"
                && s["color_space"] == "bt2020nc"
                && s["color_range"] == "tv"
        })
    {
        return err("HDR verification failed: expected 10-bit PQ / BT.2020 limited-range video. The file is retained; check GPU Screen Recorder, driver and display HDR support.");
    }
    Ok(())
}
pub struct Capture {
    pub status: watch::Sender<Value>,
    control: Option<mpsc::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    pub replay: bool,
    color: String,
    ipc: PathBuf,
}
impl Capture {
    pub fn new(replay: bool, ipc: PathBuf) -> Self {
        let status = if replay {
            json!({"state":"off","message":"Instant replay is off.","file":null})
        } else {
            json!({"state":"idle","message":"Recorder ready.","file":null})
        };
        Self {
            status: watch::channel(status).0,
            control: None,
            task: None,
            replay,
            color: "sdr".into(),
            ipc,
        }
    }
    pub fn snapshot(&self) -> Value {
        self.status.borrow().clone()
    }
    pub fn active(&self) -> bool {
        self.task.as_ref().is_some_and(|t| !t.is_finished())
    }
    fn set(&self, state: &str, message: impl AsRef<str>, file: Option<&Path>) {
        self.status
            .send_replace(json!({"state":state,"message":message.as_ref(),"file":file}));
    }
    pub fn inspect_previous(&self, cfg: &Value) {
        let orphans = orphaned();
        let folder = util::expand(cfg["recording"]["output_dir"].as_str().unwrap());
        let mut partials: Vec<_> = fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name().is_some_and(|s| {
                    let s = s.to_string_lossy();
                    s.starts_with(".Murmur_") && s.ends_with(".part.mkv")
                })
            })
            .collect();
        partials.sort();
        if let Some(pid) = orphans.first() {
            self.set("error",format!("A recorder from an earlier Murmur session is still running (PID {pid}). Stop it with: kill -INT {pid}. Its partial file stays in the output folder."),None);
        } else if !partials.is_empty() {
            self.set("idle",format!("Found {} unfinished recording(s) from an earlier session in {} (.Murmur_*.part.mkv). See docs/recording.md for recovery.",partials.len(),folder.display()),partials.first().map(PathBuf::as_path));
        }
    }
    async fn start_inner(&mut self, cfg: &Value) -> Result<Value> {
        if self.active() {
            if self.snapshot()["state"] == "stopping" {
                return err("The previous capture is still being finalized.");
            }
            return Ok(self.snapshot());
        }
        let r = &cfg["recording"];
        if !self.replay && r["configured"] != true {
            return err("Open Recording settings and finish source, audio, folder, and permission setup first.");
        }
        let monitor = r["monitor"].as_str().unwrap().trim();
        if monitor.is_empty() {
            return err("Choose a display in Recording settings first.");
        }
        let monitors = desktop::monitors().await?;
        let display = monitors
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == monitor)
            .ok_or("Capture source is unavailable. Choose a connected monitor.")?;
        self.color = color(r, display)?.into();
        let backend = if self.replay || self.color == "hdr" {
            "gpu-screen-recorder"
        } else {
            "wf-recorder"
        };
        if util::which(backend).is_none() {
            return err(format!(
                "{backend} is missing. Install it using your distribution's package manager."
            ));
        }
        if util::which("ffprobe").is_none() {
            return err("ffprobe is missing. Install FFmpeg to validate saved recordings.");
        }
        let folder = util::expand(r["output_dir"].as_str().unwrap());
        util::private_dir(&folder)?;
        let folder = fs::canonicalize(folder)?;
        let mut staging = None;
        let mut final_path = None;
        let mut argv = if self.replay {
            util::private_dir(self.ipc.parent().ok_or("Invalid replay socket")?)?;
            replay_command(cfg, &self.ipc, &folder, &self.color)
        } else {
            if let Some(pid) = orphaned().first() {
                return err(format!("A recorder from an earlier Murmur session is still running (PID {pid}). Stop it with: kill -INT {pid}"));
            }
            let mut args = if self.color == "hdr" {
                let mut a = vec![
                    backend.into(),
                    "-w".into(),
                    monitor.into(),
                    "-f".into(),
                    r["fps"].to_string(),
                    "-c".into(),
                    "mkv".into(),
                    "-v".into(),
                    "no".into(),
                ];
                a.extend(gpu_args(&self.color));
                a
            } else {
                vec![
                    backend.into(),
                    "--output".into(),
                    monitor.into(),
                    "--muxer".into(),
                    "matroska".into(),
                    "--codec".into(),
                    r["codec"].as_str().unwrap().into(),
                    "--framerate".into(),
                    r["fps"].to_string(),
                ]
            };
            if self.color == "sdr" {
                if r["codec"] == "libx264" {
                    args.extend([
                        "--pixel-format".into(),
                        "yuv420p".into(),
                        "--codec-param".into(),
                        "preset=ultrafast".into(),
                        "--codec-param".into(),
                        format!("crf={}", r["quality"]),
                        "--codec-param".into(),
                        "threads=2".into(),
                    ]);
                } else {
                    let device = r["vaapi_device"].as_str().unwrap();
                    let name = std::ffi::CString::new(device)?;
                    if unsafe { libc::access(name.as_ptr(), libc::R_OK | libc::W_OK) } != 0 {
                        return err(
                            "The selected VA-API render device is unavailable or not accessible.",
                        );
                    }
                    args.extend([
                        "--device".into(),
                        device.into(),
                        "--codec-param".into(),
                        format!("qp={}", r["quality"]),
                    ]);
                }
            }
            let system = r["system_audio"] == true;
            let mic = r["microphone"] == true;
            if system || mic {
                let device = r["audio_device"].as_str().unwrap().trim();
                if device.is_empty() {
                    return err("Choose an explicit recording audio source in Recording settings.");
                }
                if system && mic && r["combined_audio_source"] != true {
                    return err("Configure a combined audio source explicitly.");
                }
                let sources = desktop::audio_sources().await?;
                let chosen=sources.as_array().unwrap().iter().find(|s|s["name"]==device).ok_or("The chosen recording audio source is unavailable. Check Recording settings.")?;
                if !(system && mic) {
                    if system && chosen["monitor"] != true {
                        return err("System audio requires a sink monitor source; choose its .monitor source.");
                    }
                    if mic && chosen["monitor"] == true {
                        return err(
                            "Microphone recording requires an input source, not a sink monitor.",
                        );
                    }
                }
                if self.color == "hdr" {
                    args.extend(["-a".into(), device.into(), "-ac".into(), "aac".into()]);
                } else {
                    args.extend([
                        "--audio-backend".into(),
                        "pulse".into(),
                        format!("--audio={device}"),
                        "--audio-codec".into(),
                        "aac".into(),
                    ]);
                }
            }
            let tag = format!(
                "{}_{}_{:x}",
                timestamp(),
                std::process::id(),
                util::now().to_bits()
            );
            let output = folder.join(format!("Murmur_{tag}.mkv"));
            let partial = folder.join(format!(".Murmur_{tag}.part.mkv"));
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&partial)?;
            args.extend(if self.color == "hdr" {
                vec!["-o".into(), partial.to_string_lossy().into()]
            } else {
                vec![
                    "--overwrite".into(),
                    "--file".into(),
                    partial.to_string_lossy().into(),
                ]
            });
            staging = Some(partial);
            final_path = Some(output);
            args
        };
        argv[0] = util::which(&argv[0]).unwrap().to_string_lossy().into();
        let mut command = util::owned_command(&argv, libc::SIGINT, true)?;
        command.stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                if let Some(p) = &staging {
                    let _ = fs::remove_file(p);
                }
                return Err(e.into());
            }
        };
        let mut errors = child.stderr.take().ok_or("Missing capture stderr")?;
        let mut diagnostics = tokio::spawn(async move {
            let mut tail = VecDeque::with_capacity(4);
            let mut chunk = [0; 4096];
            while let Ok(n) = errors.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                if tail.len() == 4 {
                    tail.pop_front();
                }
                tail.push_back(chunk[..n].to_vec());
            }
            tail.into_iter().flatten().collect::<Vec<u8>>()
        });
        let (tx, mut rx) = mpsc::channel(1);
        self.control = Some(tx);
        let replay = self.replay;
        let status = self.status.clone();
        let color = self.color.clone();
        self.set(
            if replay { "on" } else { "recording" },
            if replay {
                format!(
                    "Instant replay on · {} · keeping the last {} seconds in memory.",
                    color.to_uppercase(),
                    cfg["replay"]["seconds"]
                )
            } else {
                format!(
                    "Recorder running · {}. Stop to finalize and verify the file.",
                    color.to_uppercase()
                )
            },
            staging.as_deref(),
        );
        self.task = Some(tokio::spawn(async move {
            let (requested, forced, code) = tokio::select! {result=child.wait()=>(false,false,result),_=rx.recv()=>{let previous=status.borrow().clone();status.send_replace(json!({"state":"stopping","message":if replay{"Stopping instant replay…"}else{"Finishing recording…"},"file":previous["file"]}));let forced=util::stop(&mut child,libc::SIGINT,if replay{5}else{12}).await.unwrap_or(true);(true,forced,child.wait().await)}};
            let tail = match timeout(Duration::from_secs(2), &mut diagnostics).await {
                Ok(result) => result.unwrap_or_default(),
                Err(_) => {
                    diagnostics.abort();
                    let _ = diagnostics.await;
                    vec![]
                }
            };
            if replay {
                status.send_replace(if requested{json!({"state":"off","message":"Instant replay is off.","file":null})}else{json!({"state":"error","message":format!("Instant replay stopped (status {:?}). {}",code,String::from_utf8_lossy(&tail).trim().chars().take(800).collect::<String>()),"file":null})});
                return;
            }
            let partial = staging.unwrap();
            let final_path = final_path.unwrap();
            let finished = async {
                if !requested || forced || !code.as_ref().is_ok_and(|c| c.success()) {
                    return err(format!(
                        "Recorder exited with status {:?}. {}",
                        code,
                        String::from_utf8_lossy(&tail)
                            .trim()
                            .chars()
                            .take(1600)
                            .collect::<String>()
                    ));
                }
                verify(&partial, &color).await?;
                fs::File::open(&partial)?.sync_all()?;
                fs::hard_link(&partial, &final_path)?;
                fs::remove_file(&partial)?;
                fs::File::open(final_path.parent().unwrap())?.sync_all()?;
                Ok(())
            }
            .await;
            match finished {
                Ok(()) => {
                    status.send_replace(json!({"state":"idle","message":format!("Saved recording: {}",final_path.file_name().unwrap().to_string_lossy()),"file":final_path}));
                }
                Err(e) => {
                    if fs::metadata(&partial).is_ok_and(|m| m.len() == 0) {
                        let _ = fs::remove_file(&partial);
                    }
                    let retained = if partial.exists() {
                        Some(partial)
                    } else if final_path.exists() {
                        Some(final_path)
                    } else {
                        None
                    };
                    let detail=retained.as_ref().map(|p|format!(" Partial video retained at {}. See docs/recording.md for recovery.",p.display())).unwrap_or_default();
                    status.send_replace(
                        json!({"state":"error","message":format!("{e}{detail}"),"file":retained}),
                    );
                }
            }
        }));
        Ok(self.snapshot())
    }
    pub async fn start(&mut self, cfg: &Value) -> Result<Value> {
        match self.start_inner(cfg).await {
            Ok(v) => Ok(v),
            Err(e) => {
                self.set("error", e.to_string(), None);
                Err(e)
            }
        }
    }
    pub async fn stop(&mut self) -> Result<Value> {
        if self.active() {
            if let Some(tx) = &self.control {
                let _ = tx.send(()).await;
            }
            if let Some(task) = self.task.take() {
                let _ = task.await;
            }
        }
        let status = self.snapshot();
        if status["state"] == "error" && !self.replay {
            return err(status["message"].as_str().unwrap_or("Recording failed"));
        }
        Ok(status)
    }
    pub async fn save(&self) -> Result<Value> {
        if !self.replay || !self.active() {
            return err("Instant replay is off. Say “start replay” first.");
        }
        let previous = self.snapshot();
        self.set(
            "on",
            "Saving the replay…",
            previous["file"].as_str().map(Path::new),
        );
        let mut path = None;
        let saved = timeout(Duration::from_secs(40), async {
            let mut stream = UnixStream::connect(&self.ipc).await.map_err(|e| {
                format!("Instant replay is still starting; try again in a moment. {e}")
            })?;
            stream
                .write_all(b"{\"id\":1,\"name\":\"save-replay\"}\n")
                .await?;
            let bytes = util::line(&mut BufReader::new(stream), 65536)
                .await?
                .ok_or("Instant replay did not answer")?;
            let reply: Value = serde_json::from_slice(&bytes)?;
            if reply["result"] != "ok" || !reply["data"].is_string() {
                return err(format!("Replay not saved: {}", reply["data"]));
            }
            let output = PathBuf::from(reply["data"].as_str().unwrap());
            path = Some(output.clone());
            verify(&output, &self.color).await?;
            Ok::<_, crate::Error>(output)
        })
        .await;
        match saved {
            Ok(Ok(path)) => {
                self.set(
                    "on",
                    format!(
                        "Saved replay: {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    Some(&path),
                );
                Ok(self.snapshot())
            }
            other => {
                let message = match other {
                    Ok(Err(e)) => e.to_string(),
                    _ => "Instant replay did not answer within its deadline".into(),
                };
                let message = if let Some(path) = path {
                    format!("{message} File retained at {}.", path.display())
                } else {
                    message
                };
                self.set("on", &message, previous["file"].as_str().map(Path::new));
                err(message)
            }
        }
    }
}
fn timestamp() -> String {
    let time = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    if unsafe { libc::localtime_r(&time, tm.as_mut_ptr()) }.is_null() {
        return time.to_string();
    }
    let tm = unsafe { tm.assume_init() };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

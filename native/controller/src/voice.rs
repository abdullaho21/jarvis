use crate::{
    err,
    policy::{wake_forms, Parser, TABLES},
    util, Result,
};
use serde_json::{json, Value};
use std::{env, fs, path::PathBuf};

pub fn project_root() -> Option<PathBuf> {
    env::var_os("MURMUR_PROJECT_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            env::current_exe()
                .ok()?
                .ancestors()
                .find(|p| p.join("pyproject.toml").is_file() && p.join("src/murmur").is_dir())
                .map(|p| p.to_path_buf())
        })
}
pub fn binary() -> Result<PathBuf> {
    if let Some(p) = env::var_os("MURMUR_RUST_VOICE_BINARY") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
        return err("Configured Rust voice worker is missing");
    }
    let mut candidates = vec![];
    if let Ok(exe) = env::current_exe() {
        candidates.push(exe.with_file_name("jarvis-voice"));
    }
    if let Some(root) = project_root() {
        candidates.extend([
            root.join("src/murmur/bin/jarvis-voice"),
            root.join("native/voice-worker/target/release/jarvis-voice"),
        ]);
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| "Rust voice worker missing. Run scripts/build_voice_worker.py first.".into())
}
pub fn vosk_library() -> Result<PathBuf> {
    if let Some(p) = env::var_os("MURMUR_VOSK_LIBRARY") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
        return err("Configured Vosk library is missing");
    }
    let mut roots = vec![];
    if let Some(root) = project_root() {
        roots.push(root.join(".venv/lib"));
    }
    if let Some(venv) = env::var_os("VIRTUAL_ENV") {
        roots.push(PathBuf::from(venv).join("lib"));
    }
    roots.extend([PathBuf::from("/usr/lib"), util::home().join(".local/lib")]);
    for root in roots {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with("python") {
                    let path = entry.path().join("site-packages/vosk/libvosk.so");
                    if path.is_file() {
                        return Ok(path);
                    }
                }
            }
        }
    }
    err("Rust voice worker requires the local Vosk library and model")
}
fn model_root() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| util::home().join(".local/share"))
        .join("murmur/models")
}
pub fn prepare(
    config: &Value,
    mode: &str,
    parser: &Parser,
    grammar: Value,
) -> Result<(PathBuf, Value)> {
    let worker = binary()?;
    if mode != "test" && config["recognition"]["backend"] == "pocketsphinx" {
        return err(
            "The Rust controller requires Vosk; select auto/vosk or use the Python controller.",
        );
    }
    let library = vosk_library();
    let model = config["recognition"]["model_path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(util::expand)
        .unwrap_or_else(|| model_root().join("vosk-model-small-en-us-0.15"));
    if mode != "test"
        && (library.is_err()
            || ![
                "am/final.mdl",
                "conf/model.conf",
                "graph/Gr.fst",
                "graph/HCLr.fst",
            ]
            .iter()
            .all(|name| model.join(name).is_file()))
    {
        return err("Rust voice worker requires the local Vosk library and model");
    }
    let whisper = model_root().join("sherpa-onnx-whisper-small.en");
    let mut files = json!({"encoder":whisper.join("small.en-encoder.int8.onnx"),"decoder":whisper.join("small.en-decoder.int8.onnx"),"tokens":whisper.join("small.en-tokens.txt")});
    if config["recognition"]["accurate"] != true
        || !files
            .as_object()
            .unwrap()
            .values()
            .all(|p| std::path::Path::new(p.as_str().unwrap()).is_file())
    {
        files = Value::Null;
    }
    let denoiser = model_root().join("gtcrn_simple.onnx");
    let denoiser = if config["recognition"]["noise_suppression"] == true && !files.is_null() {
        if !denoiser.is_file() {
            return err("Noise suppression is enabled but the GTCRN model is missing");
        }
        json!(denoiser)
    } else {
        Value::Null
    };
    let candidates = if config["recognition"]["closest_match"] == true {
        parser.candidates(&grammar)?
    } else {
        json!([])
    };
    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        / 2;
    let native = json!({"vosk_library":library.unwrap_or_default(),"model_path":model,"whisper":files,"denoiser":denoiser,"threads":threads.clamp(1,8),"candidates":candidates,"wake_forms":wake_forms(config["activation"]["wake_phrase"].as_str().unwrap())?,"greetings":TABLES["GREETINGS"],"prefixes":TABLES["POLITE_PREFIXES"],"suffixes":TABLES["POLITE_SUFFIXES"]});
    Ok((
        worker,
        json!({"config":config,"mode":mode,"grammar":grammar,"native":native}),
    ))
}
pub fn available() -> bool {
    binary().is_ok() && vosk_library().is_ok()
}
pub fn ui_argv(page: &str) -> Result<Vec<String>> {
    let python = env::var_os("MURMUR_UI_PYTHON")
        .map(PathBuf::from)
        .or_else(|| {
            project_root()
                .map(|p| p.join(".venv/bin/python"))
                .filter(|p| p.is_file())
        })
        .or_else(|| util::which("python3"))
        .ok_or("Python settings client is unavailable")?;
    util::desktop_argv(vec![
        python.to_string_lossy().into_owned(),
        "-m".into(),
        "murmur".into(),
        "ui".into(),
        "--page".into(),
        page.into(),
    ])
}

use jarvis_controller::{ipc, util, Result};
use serde_json::{json, Value};
use std::io::{self, Write};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    signal::unix::{signal, SignalKind},
    time::{sleep, timeout, Duration},
};

pub fn waybar_line(event: Option<&Value>) -> Value {
    let Some(event) = event else {
        return json!({"text":"◌ off","tooltip":"Jarvis service is not running","class":"offline"});
    };
    let status = &event["status"];
    let state = status["state"].as_str().unwrap_or("offline");
    let recording = status["recording"]["state"] == "recording";
    let mut label = if recording {
        "● REC".into()
    } else {
        format!("◉ {state}")
    };
    let mut tooltip = status["message"].as_str().unwrap_or("").to_owned();
    if status["replay"]["state"] == "on" {
        label.push_str(" ⟲");
        tooltip.push_str("\nInstant replay on");
    }
    json!({"text":label,"tooltip":tooltip,"class":if recording {"recording"}else{state}})
}
fn emit(event: Option<&Value>) -> Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(&serde_json::to_vec(&waybar_line(event))?)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}
pub async fn run() -> Result<i32> {
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut delay = 2;
    let mut offline_shown = false;
    loop {
        let connected = tokio::select! {
            _=interrupt.recv()=>return Ok(0), _=terminate.recv()=>return Ok(0),
            result=timeout(Duration::from_secs(5),async {let mut stream=ipc::connect().await?;stream.write_all(b"{\"op\":\"watch\"}\n").await?;Ok::<_,jarvis_controller::Error>(stream)})=>result,
        };
        if let Ok(Ok(stream)) = connected {
            let mut reader = BufReader::new(stream);
            loop {
                let line = tokio::select! {_=interrupt.recv()=>return Ok(0),_=terminate.recv()=>return Ok(0),line=util::line(&mut reader,65536)=>line};
                match line {
                    Ok(Some(bytes)) => match serde_json::from_slice::<Value>(&bytes) {
                        Ok(event) if event["status"].is_object() => {
                            emit(Some(&event))?;
                            delay = 2;
                            offline_shown = false;
                        }
                        _ => break,
                    },
                    _ => break,
                }
            }
        }
        if !offline_shown {
            emit(None)?;
            offline_shown = true;
        }
        tokio::select! {_=interrupt.recv()=>return Ok(0),_=terminate.recv()=>return Ok(0),_=sleep(Duration::from_secs(delay))=>{}}
        delay = (delay * 2).min(30);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recording_replay_and_offline() {
        assert_eq!(waybar_line(None)["class"], "offline");
        assert_eq!(
            waybar_line(Some(
                &json!({"status":{"state":"listening","message":"Ready","recording":{"state":"recording"},"replay":{"state":"on"}}})
            )),
            json!({"text":"● REC ⟲","tooltip":"Ready\nInstant replay on","class":"recording"})
        );
    }
}

//! DDC/CI control with stable EDID identities and verified writes.
use crate::{err, util, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;
async fn query(args: &[&str]) -> Result<String> {
    let program = util::which("ddcutil")
        .ok_or("Install ddcutil and enable DDC/CI on your monitors to control brightness")?;
    let mut argv = vec![program.to_string_lossy().into_owned()];
    argv.extend(args.iter().map(|s| s.to_string()));
    util::query(&argv, 12).await
}
pub fn parse_detect(text: &str) -> Vec<Value> {
    static BUS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)I2C bus:\s*/dev/i2c-(\d+)\s*$").unwrap());
    let mut result = vec![];
    static HEADER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^(?:Display \d+|Invalid display)\s*$").unwrap());
    for block in HEADER.split(text).skip(1) {
        let Some(bus) = BUS.captures(block).and_then(|c| c[1].parse::<u32>().ok()) else {
            continue;
        };
        let Some(identity) = block
            .lines()
            .find_map(|l| l.trim().strip_prefix("Monitor:"))
        else {
            continue;
        };
        let parts: Vec<_> = identity.trim().splitn(3, ':').map(str::trim).collect();
        if parts.len() != 3 || parts[0].is_empty() || parts[1].is_empty() {
            continue;
        }
        let stable = !parts[2].trim_matches('0').is_empty()
            && !["unspecified", "unknown"].contains(&parts[2].to_lowercase().as_str());
        let id = if stable {
            format!("edid:{}", parts.join(":"))
        } else {
            format!("bus:{bus}:{}:{}", parts[0], parts[1])
        };
        result.push(json!({"id":id,"bus":bus,"model":parts[1],"serial":parts[2],"stable":stable,"label":format!("{} {} ({})",parts[0],parts[1],if parts[2].is_empty(){"no serial"}else{parts[2]})}));
    }
    result
}
async fn discover() -> Result<Vec<Value>> {
    let mut monitors = parse_detect(&query(&["detect", "--brief"]).await?);
    for i in 0..monitors.len() {
        monitors[i]["ambiguous"] = json!(
            monitors
                .iter()
                .filter(|m| m["id"] == monitors[i]["id"])
                .count()
                > 1
        );
    }
    Ok(monitors)
}
async fn read(bus: u64) -> Result<(u64, u64)> {
    let output = query(&["getvcp", "10", "--terse", "--bus", &bus.to_string()]).await?;
    let words: Vec<_> = output.split_whitespace().collect();
    if words.len() == 5 && words[..3] == ["VCP", "10", "C"] {
        if let (Ok(current), Ok(maximum)) = (words[3].parse::<u64>(), words[4].parse::<u64>()) {
            if maximum > 0 && maximum <= 65535 && current <= maximum {
                return Ok((current, maximum));
            }
        }
    }
    err("This monitor does not report a usable brightness control")
}
pub async fn inventory() -> Result<Vec<Value>> {
    let mut monitors = discover().await?;
    for monitor in &mut monitors {
        match read(monitor["bus"].as_u64().unwrap()).await {
            Ok((current, maximum)) => {
                monitor["percent"] = json!((current * 100 + maximum / 2) / maximum)
            }
            Err(error) => monitor["error"] = json!(error.to_string()),
        }
    }
    Ok(monitors)
}
pub async fn execute(intent: &Value, config: &Value) -> Result<String> {
    let identity = intent["target"]["monitor"]
        .as_str()
        .ok_or("Invalid monitor")?;
    let setting = config["brightness"]["monitors"]
        .get(identity)
        .ok_or("Monitor is not configured")?;
    let monitors = discover().await?;
    let matches: Vec<_> = monitors.iter().filter(|m| m["id"] == identity).collect();
    if matches.len() != 1 {
        return err("Monitor is disconnected or its identity is ambiguous; refresh Displays");
    }
    let bus = matches[0]["bus"].as_u64().unwrap();
    let (current, maximum) = read(bus).await?;
    let percent = if intent["action"] == "brightness_set" {
        intent["target"]["percent"]
            .as_u64()
            .filter(|n| *n <= 100)
            .ok_or("Brightness must be a whole percentage from zero to 100")?
    } else {
        let step = config["brightness"]["step"]
            .as_i64()
            .ok_or("Invalid brightness step")?;
        (((current * 100 + maximum / 2) / maximum) as i64
            + if intent["action"] == "brightness_down" {
                -step
            } else {
                step
            })
        .clamp(0, 100) as u64
    };
    let raw = (maximum * percent + 50) / 100;
    query(&["setvcp", "10", &raw.to_string(), "--bus", &bus.to_string()]).await?;
    Ok(format!(
        "{} brightness set to {percent}%",
        setting["name"].as_str().unwrap_or("Monitor")
    ))
}

//! Exact, configured shortcuts shared by the controller and recognizer.
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

pub fn normalize(s: &str) -> String {
    s.nfkc()
        .case_fold()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(|w| if w == "fullscreen" { "full screen" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}
const SMALL: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

pub fn number(n: usize) -> String {
    if n < 20 {
        SMALL[n].into()
    } else if n == 100 {
        "one hundred".into()
    } else if n.is_multiple_of(10) {
        TENS[n / 10].into()
    } else {
        format!("{} {}", TENS[n / 10], SMALL[n % 10])
    }
}

fn percentage(text: &str) -> Option<usize> {
    if text == "one hundred" {
        return Some(100);
    }
    if let Some(n) = SMALL.iter().position(|&word| word == text) {
        return Some(n);
    }
    let (tens, ones) = text.split_once(' ').unwrap_or((text, ""));
    let n = TENS
        .iter()
        .position(|&word| word == tens)
        .filter(|&n| n >= 2)?;
    let unit = if ones.is_empty() {
        0
    } else {
        SMALL[1..10].iter().position(|&word| word == ones)? + 1
    };
    Some(n * 10 + unit)
}
pub fn numeric(text: &str) -> String {
    text.split_whitespace()
        .map(|w| {
            w.parse::<usize>()
                .ok()
                .filter(|n| *n <= 100)
                .map_or_else(|| w.to_owned(), number)
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn brightness_like(text: &str) -> bool {
    text.split_whitespace()
        .any(|w| ["brightness", "brighten", "dim"].contains(&w))
}
#[derive(Clone, Debug, Default)]
pub struct Rules {
    pub launches: Map<String, Value>,
    pub monitors: Map<String, Value>,
}
impl Rules {
    pub fn from_config(c: &Value) -> Result<Self, String> {
        let mut rules = Self::default();
        let aliases = c["aliases"]
            .as_object()
            .ok_or("aliases must be an object")?;
        let prefixes = c["app_prefixes"]
            .as_object()
            .ok_or("app_prefixes must be an object")?;
        if prefixes.len() > 256 {
            return Err("Too many application prefixes".into());
        }
        let mut seen = BTreeSet::new();
        for (alias, prefix) in prefixes {
            let name = normalize(alias);
            let prefix = prefix
                .as_str()
                .ok_or("Application launch word must be text")?;
            if !seen.insert(name.clone())
                || prefix.chars().count() > 40
                || prefix.chars().any(char::is_control)
                || prefix.split_whitespace().count() > 4
            {
                return Err("Invalid or duplicate application prefix".into());
            }
            let _target = aliases
                .iter()
                .find(|(k, _)| normalize(k) == name)
                .map(|(_, v)| v)
                .ok_or("Launch words need an existing application alias")?;
            let p = normalize(prefix);
            let phrase = if p.is_empty() {
                name.clone()
            } else {
                format!("{p} {name}")
            };
            if brightness_like(&phrase) || rules.launches.insert(phrase, json!(name)).is_some() {
                return Err("Conflicting application shortcut".into());
            }
        }
        let b = &c["brightness"];
        let monitors = b["monitors"]
            .as_object()
            .ok_or("Brightness monitors must be an object")?;
        let default = b["default"]
            .as_str()
            .ok_or("Default monitor must be text")?;
        if monitors.len() > 8 || (!default.is_empty() && !monitors.contains_key(default)) {
            return Err("Choose a configured default monitor (up to eight monitors)".into());
        }
        if !b["step"].as_u64().is_some_and(|n| (1..=100).contains(&n)) {
            return Err("Brightness step must be 1–100".into());
        }
        let mut names = BTreeSet::new();
        for (id, monitor) in monitors {
            if id.is_empty()
                || id.len() > 512
                || id.chars().any(char::is_control)
                || !monitor.is_object()
                || monitor
                    .as_object()
                    .unwrap()
                    .keys()
                    .any(|k| !["name", "aliases"].contains(&k.as_str()))
            {
                return Err("Invalid monitor settings".into());
            }
            let name = monitor["name"]
                .as_str()
                .ok_or("Monitor name must be text")?;
            let aliases = monitor["aliases"]
                .as_array()
                .ok_or("Monitor aliases must be a list")?;
            for value in
                std::iter::once(name).chain(aliases.iter().map(|v| v.as_str().unwrap_or("")))
            {
                let n = numeric(&normalize(value));
                if n.is_empty()
                    || value.chars().count() > 40
                    || value.chars().any(char::is_control)
                    || n.split_whitespace().any(|w| {
                        [
                            "brightness",
                            "brighten",
                            "dim",
                            "percent",
                            "to",
                            "by",
                            "the",
                            "main",
                        ]
                        .contains(&w)
                    })
                    || !names.insert(n.clone())
                {
                    return Err("Choose distinct short monitor names without command words".into());
                }
                rules.monitors.insert(n, json!(id));
            }
        }
        if names.len() > 8 {
            return Err("Use at most eight monitor names and aliases in total".into());
        }
        if !default.is_empty() {
            rules.monitors.insert("main".into(), json!(default));
        }
        Ok(rules)
    }
    pub fn from_grammar(g: &Value) -> Self {
        Self {
            launches: g["launches"].as_object().cloned().unwrap_or_default(),
            monitors: g["brightness_names"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
        }
    }
    pub fn parse(&self, text: &str) -> Option<Value> {
        self.parse_normalized(&normalize(text))
    }
    /// Parse normalized text without expanding every monitor/percentage pair.
    pub fn parse_normalized(&self, p: &str) -> Option<Value> {
        if let Some(target) = self.launches.get(p) {
            return Some(json!({"action":"open_app","target":target}));
        }
        for (shortcut, alias) in &self.launches {
            if let Some(alias) = alias.as_str() {
                if let Some(prefix) = shortcut.strip_suffix(alias) {
                    if p == format!("{prefix}the {alias}") {
                        return Some(json!({"action":"open_app","target":alias}));
                    }
                }
            }
        }
        if !brightness_like(p) {
            return None;
        }
        let p = numeric(p);
        let (verb, target) = p.split_once(' ').unwrap_or((&p, ""));
        let target = target.strip_prefix("the ").unwrap_or(target);
        let (name, value) = match verb {
            "brighten" | "dim" => {
                let name = if target.is_empty() { "main" } else { target };
                if let Some(id) = self.monitors.get(name) {
                    return Some(json!({
                        "action": if verb == "dim" { "brightness_down" } else { "brightness_up" },
                        "target": { "monitor": id }
                    }));
                }
                target.rsplit_once(" to ")?
            }
            "set" => {
                if let Some(value) = target.strip_prefix("brightness to ") {
                    ("main", value)
                } else {
                    target.rsplit_once(" brightness to ")?
                }
            }
            _ => return None,
        };
        let id = self.monitors.get(name)?;
        let n = percentage(value.strip_suffix(" percent").unwrap_or(value))?;
        Some(json!({"action":"brightness_set","target":{"monitor":id,"percent":n}}))
    }
    pub fn phrases(&self) -> Vec<String> {
        let mut p: Vec<_> = self.launches.keys().cloned().collect();
        for name in self.monitors.keys() {
            p.extend([format!("brighten {name}"), format!("dim {name}")]);
            for n in 0..=100 {
                p.push(format!("set {name} brightness to {} percent", number(n)));
            }
        }
        p.sort();
        p
    }
    pub fn canonical(&self, text: &str) -> Option<String> {
        let intent = self.parse(text)?;

        // One canonical name prevents synonyms competing with each other.
        if intent["action"] == "open_app" {
            return self
                .launches
                .iter()
                .find(|(_, id)| **id == intent["target"])
                .map(|(p, _)| p.clone());
        }
        let name = self
            .monitors
            .iter()
            .filter(|(_, id)| **id == intent["target"]["monitor"])
            .min_by_key(|(name, _)| *name)?
            .0;
        Some(match intent["action"].as_str()? {
            "brightness_set" => format!(
                "set {name} brightness to {} percent",
                number(intent["target"]["percent"].as_u64()? as usize)
            ),
            "brightness_up" => format!("brighten {name}"),
            _ => format!("dim {name}"),
        })
    }
}
/// Reject signed/fractional brightness values before punctuation normalization.
pub fn invalid_number(text: &str) -> bool {
    static BAD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"[+−–—﹣－＋-]\s*\d|\d[.,]\d|\b(minus|negative|point)\b").unwrap()
    });
    BAD.is_match(&text.to_lowercase())
}

use crate::inference::{Decoder, Model, Transcriber};
use crate::matching::{phrase, Matcher};
use crate::{NativeConfig, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

#[derive(Default, Deserialize)]
pub struct Grammar {
    #[serde(default)]
    pub phrases: Vec<String>,
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub verbs: Vec<String>,
    #[serde(default)]
    pub prefixes: Vec<String>,
    #[serde(default)]
    pub overflow: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Recognition {
    pub kind: String,
    pub text: String,
    pub confidence: Option<f64>,
    pub min_word_posterior: Option<f64>,
    pub heard: Option<String>,
    pub closest_match: bool,
}
impl Recognition {
    pub(crate) fn new(kind: &str, text: String, confidence: Option<f64>) -> Self {
        Self {
            kind: kind.into(),
            text,
            confidence,
            min_word_posterior: None,
            heard: None,
            closest_match: false,
        }
    }
}

pub struct Engine {
    // Decoders retain their shared model, so all handle destruction is ordered.
    wake: Decoder,
    command: Decoder,
    open: Option<Decoder>,
    transcriber: Option<Transcriber>,
    matcher: Matcher,
    sounds: HashMap<String, String>,
    wake_name: Vec<String>,
    wake_phrase: String,
    sensitivity: f64,
    mode: Option<&'static str>,
    preroll: VecDeque<[u8; 960]>,
    hangover: usize,
    utterance_bytes: usize,
    open_text: Vec<String>,
    wake_audio: Vec<u8>,
    stream_bytes: usize,
    wake_start: usize,
    pending: Option<Recognition>,
    command_audio: Vec<u8>,
    speech_frames: usize,
    quiet_run: usize,
    pub unsupported: Vec<String>,
    pub unmatched: Option<String>,
    pub name: String,
}

fn token(word: &str) -> bool {
    let pieces: Vec<_> = word.split(['\'', '-']).collect();
    !pieces.is_empty()
        && pieces
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase()))
}

fn supported(model: &Model, phrases: &[String]) -> Vec<String> {
    phrases
        .iter()
        .map(|s| phrase(s))
        .filter(|s| !s.is_empty() && s.split_whitespace().all(|w| token(w) && model.knows(w)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn text(result: &Value) -> String {
    result["text"]
        .as_str()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn confidence(result: &Value) -> Option<f64> {
    result["result"].as_array().and_then(|words| {
        words
            .iter()
            .filter_map(|w| w["conf"].as_f64())
            .min_by(f64::total_cmp)
    })
}

impl Engine {
    pub fn new(native: &NativeConfig, grammar: &Grammar, config: &Value) -> Result<Self> {
        // Compile Whisper before loading resident Vosk state, so its one-time
        // model-import scratch does not overlap Vosk's steady allocation.
        let transcriber = if native.whisper.is_some() {
            Some(Transcriber::new(native)?)
        } else {
            None
        };
        let model = Model::load(&native.vosk_library, &native.model_path)?;
        let known = supported(&model, &grammar.phrases);
        let apps = supported(&model, &grammar.apps);
        let mut verbs = supported(&model, &grammar.verbs);
        let prefixes = supported(&model, &grammar.prefixes);
        if known.is_empty() && apps.is_empty() {
            return Err("No command phrases use words the Vosk model knows".into());
        }
        let mut unsupported = grammar.overflow.clone();
        for p in grammar
            .phrases
            .iter()
            .map(|s| phrase(s))
            .collect::<BTreeSet<_>>()
        {
            if !known.contains(&p) {
                unsupported.push(p);
            }
        }
        for a in grammar
            .apps
            .iter()
            .map(|s| phrase(s))
            .collect::<BTreeSet<_>>()
        {
            if !apps.contains(&a) {
                unsupported.push(format!("open {a}"));
            }
        }
        let mut sounds = HashMap::new();
        for name in grammar
            .apps
            .iter()
            .map(|s| phrase(s))
            .collect::<BTreeSet<_>>()
        {
            if apps.contains(&name) || !name.split_whitespace().all(token) {
                continue;
            }
            let mut parts = Vec::new();
            for word in name.split_whitespace() {
                if model.knows(word) {
                    parts.push(word.to_owned());
                    continue;
                }
                let split = (3..=word.len().saturating_sub(2)).rev().find(|&cut| {
                    word.len() - cut >= if cut >= 5 { 2 } else { 3 }
                        && model.knows(&word[..cut])
                        && model.knows(&word[cut..])
                });
                if let Some(cut) = split {
                    parts.push(format!("{} {}", &word[..cut], &word[cut..]));
                } else {
                    parts.clear();
                    break;
                }
            }
            if !parts.is_empty() {
                let spoken = parts.join(" ");
                if spoken != name && !apps.contains(&spoken) {
                    sounds.entry(spoken).or_insert(name);
                }
            }
        }
        unsupported.retain(|p| {
            !sounds
                .values()
                .any(|name| p.strip_prefix("open ").unwrap_or(p) == name)
        });
        if !native.candidates.is_empty() {
            let reachable: HashSet<_> = grammar
                .apps
                .iter()
                .map(|a| format!("open {}", phrase(a)))
                .collect();
            unsupported.retain(|p| !reachable.contains(p));
        }
        let mut spoken_apps = apps;
        spoken_apps.extend(sounds.keys().cloned());
        let mut full: BTreeSet<String> = known.iter().cloned().collect();
        for v in &verbs {
            for a in &spoken_apps {
                full.insert(format!("{v} {a}"));
            }
        }
        for p in &prefixes {
            for k in &known {
                full.insert(format!("{p} {k}"));
            }
        }
        let full: Vec<_> = full.into_iter().collect();
        let wake_forms = supported(&model, &native.wake_forms);
        if wake_forms.is_empty() {
            return Err(
                "Wake phrase uses words the Vosk model doesn't know; choose another".into(),
            );
        }
        let wake_phrase = phrase(
            config["activation"]["wake_phrase"]
                .as_str()
                .ok_or("Missing wake phrase")?,
        );
        let mut wake_name: Vec<String> =
            wake_phrase.split_whitespace().map(str::to_owned).collect();
        if wake_name.len() > 1 && native.greetings.contains(&wake_name[0]) {
            wake_name.remove(0);
        }
        let mut wake_grammar = wake_forms.clone();
        wake_grammar.extend(full.iter().map(|c| format!("{} {c}", wake_forms[0])));
        let wake = Decoder::new(model.clone(), Some(wake_grammar), true)?;
        let command = Decoder::new(model.clone(), Some(full), true)?;
        let open = if config["recognition"]["closest_match"].as_bool() == Some(true) {
            Some(Decoder::new(model, None, false)?)
        } else {
            None
        };
        let mut matcher_apps: HashSet<_> = spoken_apps
            .into_iter()
            .chain(sounds.values().cloned())
            .collect();
        if verbs.is_empty() {
            matcher_apps.clear();
        }
        verbs.sort_by_key(|v| std::cmp::Reverse(v.len()));
        let matcher = Matcher {
            phrases: known.into_iter().collect(),
            apps: matcher_apps,
            verbs,
            prefixes: native.prefixes.clone(),
            suffixes: native.suffixes.clone(),
            candidates: native.candidates.clone(),
        };
        Ok(Self {
            wake,
            command,
            open,
            transcriber,
            matcher,
            sounds,
            wake_name,
            wake_phrase,
            sensitivity: config["activation"]["sensitivity"]
                .as_f64()
                .ok_or("Missing sensitivity")?,
            mode: None,
            preroll: VecDeque::new(),
            hangover: 0,
            utterance_bytes: 0,
            open_text: vec![],
            wake_audio: vec![],
            stream_bytes: 0,
            wake_start: 0,
            pending: None,
            command_audio: vec![],
            speech_frames: 0,
            quiet_run: 0,
            unsupported,
            unmatched: None,
            name: if native.whisper.is_some() {
                "vosk+whisper-small"
            } else {
                "vosk"
            }
            .into(),
        })
    }
    fn canonical(&self, text: &str) -> String {
        for verb in &self.matcher.verbs {
            if let Some(name) = text
                .strip_prefix(&format!("{verb} "))
                .and_then(|a| self.sounds.get(a))
            {
                return format!("{verb} {name}");
            }
        }
        text.to_owned()
    }
    fn reset(&mut self) {
        self.wake.reset();
        self.command.reset();
        if let Some(open) = &self.open {
            open.reset();
        }
        self.preroll.clear();
        self.hangover = 0;
        self.utterance_bytes = 0;
        self.open_text.clear();
        self.wake_audio.clear();
        self.wake_start = self.stream_bytes;
        self.command_audio.clear();
        self.speech_frames = 0;
        self.quiet_run = 0;
    }
    pub fn begin(&mut self, command: bool) {
        self.reset();
        self.pending = None;
        self.unmatched = None;
        self.mode = Some(if command { "command" } else { "wake" });
    }
    pub fn after_wake(&mut self) -> Option<Recognition> {
        self.pending.take()
    }
    pub fn feed(&mut self, pcm: &[u8], quiet: bool) -> Result<Option<Recognition>> {
        if pcm.len() != 960 {
            return Err("Expected exactly 960 PCM bytes".into());
        }
        if self.mode.is_none() {
            return Ok(None);
        }
        if quiet && self.hangover == 0 {
            if self.preroll.len() == 10 {
                self.preroll.pop_front();
            }
            self.preroll
                .push_back(pcm.try_into().expect("frame size checked"));
            return Ok(None);
        }
        if !quiet {
            let preroll = self.hangover == 0;
            self.hangover = 40;
            if preroll {
                while let Some(frame) = self.preroll.pop_front() {
                    if let Some(result) = self.decode(&frame)? {
                        return Ok(Some(result));
                    }
                }
            }
        } else {
            self.hangover -= 1;
        }
        if let Some(result) = self.decode(pcm)? {
            return Ok(Some(result));
        }
        if self.mode == Some("command") && self.transcriber.is_some() {
            if quiet {
                self.quiet_run += 1;
            } else {
                self.speech_frames += 1;
                self.quiet_run = 0;
            }
            if self.speech_frames >= 8 && self.quiet_run >= 20 {
                return self.flush();
            }
        }
        Ok(None)
    }
    pub fn flush(&mut self) -> Result<Option<Recognition>> {
        if self.mode != Some("command") || self.command_audio.is_empty() {
            return Ok(None);
        }
        let result = self.command.result(true)?;
        self.command_result(result)
    }
    fn decode(&mut self, pcm: &[u8]) -> Result<Option<Recognition>> {
        self.utterance_bytes += pcm.len();
        if self.utterance_bytes > 320000 {
            self.reset();
            return Ok(None);
        }
        if self.mode == Some("wake") {
            self.wake_audio.extend_from_slice(pcm);
            self.stream_bytes += pcm.len();
            if self.wake.accept(pcm)? {
                self.utterance_bytes = 0;
                let audio = std::mem::take(&mut self.wake_audio);
                let start = self.wake_start;
                self.wake_start = self.stream_bytes;
                let result = self.wake.result(false)?;
                return self.wake_result(result, &audio, start);
            }
            return Ok(None);
        }
        self.command_audio.extend_from_slice(pcm);
        if let Some(open) = &self.open {
            if open.accept(pcm)? {
                self.open_text.push(text(&open.result(false)?));
            }
        }
        if self.command.accept(pcm)? {
            self.utterance_bytes = 0;
            return self.command_result(self.command.result(false)?);
        }
        Ok(None)
    }
    fn wake_result(
        &mut self,
        result: Value,
        audio: &[u8],
        start: usize,
    ) -> Result<Option<Recognition>> {
        let words: Vec<_> = result["result"]
            .as_array()
            .map(|w| {
                w.iter()
                    .filter(|w| w["word"].as_str().is_some_and(|s| !s.is_empty()))
                    .collect()
            })
            .unwrap_or_default();
        let spoken: Vec<_> = words.iter().map(|w| w["word"].as_str().unwrap()).collect();
        let size = self.wake_name.len();
        let Some(position) = (0..3).find(|&i| {
            spoken.get(i..i + size).is_some_and(|s| {
                s.iter()
                    .copied()
                    .eq(self.wake_name.iter().map(String::as_str))
            })
        }) else {
            return Ok(None);
        };
        let name = &words[position..position + size];
        if name
            .iter()
            .filter_map(|w| w["conf"].as_f64())
            .any(|c| c < 0.8 * (1.0 - self.sensitivity))
        {
            return Ok(None);
        }
        let rest = &words[position + size..];
        self.pending = None;
        if !rest.is_empty() {
            let text = self.canonical(
                &rest
                    .iter()
                    .map(|w| w["word"].as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            let conf = rest
                .iter()
                .filter_map(|w| w["conf"].as_f64())
                .min_by(f64::total_cmp);
            let cut = ((name.last().unwrap()["end"].as_f64().unwrap_or(0.0) * 16000.0) as usize
                * 2)
            .saturating_sub(start);
            let tail = &audio[(cut - cut % 2).min(audio.len())..];
            if !text.contains("[unk]")
                && self.matcher.accepts(&text)
                && conf.is_none_or(|c| c >= 0.6)
            {
                self.pending = self.command_from_audio(
                    tail,
                    Some(serde_json::json!({"text":text,"result":rest})),
                )?;
            } else if !audio.is_empty() {
                self.pending = self.command_from_audio(tail, None)?;
            }
        }
        self.reset();
        self.mode = None;
        Ok(Some(Recognition::new(
            "wake",
            self.wake_phrase.clone(),
            None,
        )))
    }
    fn command_from_audio(
        &mut self,
        audio: &[u8],
        strict: Option<Value>,
    ) -> Result<Option<Recognition>> {
        if strict.is_none() && audio.len() < 6400 {
            return Ok(None);
        }
        self.command.reset();
        self.open_text.clear();
        self.command_audio = audio.to_vec();
        if let Some(open) = &self.open {
            open.reset();
            open.accept(audio)?;
        }
        let strict = if let Some(s) = strict {
            s
        } else {
            self.command.accept(audio)?;
            self.command.result(true)?
        };
        self.command_result(strict)
    }
    fn command_result(&mut self, result: Value) -> Result<Option<Recognition>> {
        let text = self.canonical(&text(&result));
        if let Some(open) = &self.open {
            self.open_text.push(text_of(&open.result(true)?));
            open.reset();
        }
        let mut heard = self
            .open_text
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        self.open_text.clear();
        let audio = std::mem::take(&mut self.command_audio);
        self.speech_frames = 0;
        self.quiet_run = 0;
        let mut heard_match = self.matcher.closest(&heard);
        if let Some(transcriber) = &self.transcriber {
            if !audio.is_empty() {
                let accurate = phrase(&transcriber.transcribe(&audio)?);
                let accurate_match = self.matcher.closest(&accurate);
                if accurate_match.is_some() || heard_match.is_none() {
                    if !accurate.is_empty() {
                        heard = accurate;
                    }
                    heard_match = accurate_match;
                }
            }
        }
        if let Some(recognition) =
            self.matcher
                .choose(&text, confidence(&result), &heard, heard_match)
        {
            self.mode = None;
            return Ok(Some(recognition));
        }
        let heard = if heard.is_empty() {
            text.replace("[unk]", "…").trim().to_owned()
        } else {
            heard
        };
        self.unmatched = if heard.is_empty() { None } else { Some(heard) };
        Ok(None)
    }
}

// Avoid shadowing the text() helper with the canonical command local.
fn text_of(result: &Value) -> String {
    text(result)
}

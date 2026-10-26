//! The existing command matching policy, with Python SequenceMatcher's
//! no-junk longest-block rule. The daemon still validates and dispatches text.
use std::collections::{HashMap, HashSet};

pub fn phrase(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', '!', '?'])
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn wording(text: &str, prefixes: &[String], suffixes: &[String]) -> String {
    strip_courtesy(&jarvis_phrases::normalize(text), prefixes, suffixes)
}

fn strip_courtesy(text: &str, prefixes: &[String], suffixes: &[String]) -> String {
    let mut result = text;
    if let Some(p) = prefixes
        .iter()
        .filter(|p| text.starts_with(&format!("{p} ")))
        .max_by_key(|p| p.len())
    {
        result = &result[p.len() + 1..];
    }
    if let Some(s) = suffixes
        .iter()
        .filter(|s| result.ends_with(&format!(" {s}")))
        .max_by_key(|s| s.len())
    {
        result = &result[..result.len() - s.len() - 1];
    }
    result.to_owned()
}

pub struct Matcher {
    pub phrases: HashSet<String>,
    pub apps: HashSet<String>,
    pub verbs: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub candidates: Vec<(String, String)>,
    pub exact_only: bool,
    pub rules: jarvis_phrases::Rules,
    pub blocked_apps: HashSet<String>,
}

impl Matcher {
    /// Strict grammar and free speech must agree, unless the latter clears the
    /// same 0.9 override threshold as the Python worker.
    pub fn choose(
        &self,
        text: &str,
        confidence: Option<f64>,
        heard: &str,
        heard_match: Option<(String, f64)>,
    ) -> Option<crate::engine::Recognition> {
        use crate::engine::Recognition;
        if jarvis_phrases::brightness_like(text) {
            let (p, score) = heard_match?;
            if !jarvis_phrases::brightness_like(&p) {
                return None;
            }
            return Some(Recognition::new("command", p, Some(score)));
        }
        if !text.is_empty() && !text.contains("[unk]") && self.accepts(text) {
            let strict_intent = self.intent(&wording(text, &self.prefixes, &self.suffixes));
            let disagreement = heard_match
                .as_ref()
                .is_some_and(|(p, _)| strict_intent.is_some() && self.intent(p) != strict_intent);
            if disagreement {
                if heard_match.as_ref()?.1 < 0.9 {
                    return None;
                }
            } else if confidence.is_none_or(|c| c >= 0.6) {
                let boosted = if strict_intent.is_some() {
                    heard_match
                        .as_ref()
                        .and_then(|(_, s)| confidence.map(|c| c.max(*s)))
                        .or(confidence)
                } else {
                    confidence
                };
                let mut result = Recognition::new("command", text.to_owned(), boosted);
                result.min_word_posterior = confidence;
                return Some(result);
            }
        }
        if let Some((phrase, score)) = heard_match {
            let mut result = Recognition::new("command", phrase, Some(score));
            result.heard = Some(heard.to_owned());
            result.closest_match = true;
            return Some(result);
        }
        None
    }
    pub fn accepts(&self, text: &str) -> bool {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        self.core(&text) || self.core(&strip_courtesy(&text, &self.prefixes, &self.suffixes))
    }
    fn core(&self, text: &str) -> bool {
        if self.rules.parse(text).is_some() {
            return true;
        }
        self.phrases.contains(text)
            || self.verbs.iter().any(|v| {
                text.strip_prefix(&format!("{v} ")).is_some_and(|a| {
                    let a = a.strip_prefix("the ").unwrap_or(a);
                    self.apps.contains(a)
                        && (!self.blocked_apps.contains(a)
                            || ["focus", "switch to"].contains(&v.as_str()))
                })
            })
    }
    pub fn intent(&self, text: &str) -> Option<&str> {
        self.candidates
            .iter()
            .find(|(p, _)| p == text)
            .map(|(_, i)| i.as_str())
    }
    pub fn closest(&self, heard: &str) -> Option<(String, f64)> {
        let mut heard = heard.to_owned();
        if heard.chars().count() > 256 {
            heard = heard.chars().take(257).collect();
            heard = heard
                .rsplit_once(' ')
                .map_or(heard.clone(), |(p, _)| p.to_owned());
        }
        let literal = jarvis_phrases::normalize(&heard);
        if !jarvis_phrases::brightness_like(&literal) {
            if let Some(p) = self.rules.canonical(&literal) {
                return self.intent(&p).map(|_| (p, 1.0));
            }
            if self.intent(&literal).is_some() {
                return Some((literal, 1.0));
            }
        }
        let mut text = wording(&heard, &self.prefixes, &self.suffixes);
        if jarvis_phrases::brightness_like(&text) {
            if jarvis_phrases::invalid_number(&heard) {
                return None;
            }
            let p = self.rules.canonical(&text)?;
            return self.intent(&p).map(|_| (p, 1.0));
        }
        if let Some(p) = self.rules.canonical(&text) {
            return self.intent(&p).map(|_| (p, 1.0));
        }
        for verb in &self.verbs {
            if let Some(rest) = text.strip_prefix(&format!("{verb} the ")) {
                text = format!("{verb} {rest}");
                break;
            }
        }
        if self.intent(&text).is_some() {
            return Some((text, 1.0));
        }
        if text.chars().count() > 256 {
            return None;
        }
        let numbers = "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty".split_whitespace().collect::<Vec<_>>();
        let text = text
            .split_whitespace()
            .map(|w| {
                if w.len() <= 2 && w.bytes().all(|c| c.is_ascii_digit()) {
                    w.parse::<usize>()
                        .ok()
                        .filter(|&n| n <= 20)
                        .map_or(w, |n| numbers[n])
                } else {
                    w
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        if self.intent(&text).is_some() {
            return Some((text, 1.0));
        }
        if self.exact_only {
            return None;
        }
        let whole = text.replace(' ', "");
        if whole.chars().count() < 3 {
            return None;
        }
        if let Some(m) = self.closest_windows(&[whole]) {
            return Some(m);
        }
        let words: Vec<_> = text.split_whitespace().collect();
        let mut stretches = HashSet::new();
        for start in 0..3.min(words.len()) {
            for end in start + 1..=words.len() {
                let s = words[start..end].join("");
                if s.chars().count() >= 3 {
                    stretches.insert(s);
                }
            }
        }
        let mut windows: Vec<_> = stretches.into_iter().collect();
        windows.sort();
        self.closest_windows(&windows)
    }
    fn closest_windows(&self, windows: &[String]) -> Option<(String, f64)> {
        let windows: Vec<Vec<char>> = windows.iter().map(|s| s.chars().collect()).collect();
        let letters: Vec<_> = windows.iter().map(|w| counts(w)).collect();
        let mut intents: HashMap<&str, (f64, &str)> = HashMap::new();
        for (candidate, intent) in &self.candidates {
            if intent.starts_with("brightness_") {
                continue;
            }
            let target: Vec<char> = candidate.chars().filter(|&c| c != ' ').collect();
            let target_letters = counts(&target);
            let mut best: f64 = 0.0;
            for (w, counts) in windows.iter().zip(&letters) {
                let longest = target.len().max(w.len()) as f64;
                if longest == 0.0
                    || target.len().abs_diff(w.len()) as f64 > longest * (1.0 - 0.82) * 2.0
                    || (target.len().min(w.len()) as f64) < longest * (0.82 - 0.06)
                {
                    continue;
                }
                let shared: usize = counts
                    .iter()
                    .map(|(c, n)| (*n).min(*target_letters.get(c).unwrap_or(&0)))
                    .sum();
                let bound = shared as f64 / longest;
                if bound < 0.82 - 0.06 || bound <= best {
                    continue;
                }
                best = best.max(matching_size(w, &target) as f64 / longest);
            }
            if best > intents.get(intent.as_str()).map_or(0.0, |x| x.0) {
                intents.insert(intent, (best, candidate));
            }
        }
        let mut ranked: Vec<_> = intents.values().copied().collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(b.1)));
        let &(score, name) = ranked.first()?;
        let rival = ranked.get(1).map_or(0.0, |x| x.0);
        (score >= 0.82 && score - rival >= 0.06)
            .then(|| (name.to_owned(), (score * 1000.0).round_ties_even() / 1000.0))
    }
}

fn counts(chars: &[char]) -> HashMap<char, usize> {
    let mut result = HashMap::new();
    for &c in chars {
        *result.entry(c).or_default() += 1;
    }
    result
}

fn matching_size(a: &[char], b: &[char]) -> usize {
    let mut todo = vec![(0, a.len(), 0, b.len())];
    let mut total = 0;
    while let Some((alo, ahi, blo, bhi)) = todo.pop() {
        let (mut ai, mut bj, mut size) = (alo, blo, 0);
        // Carry the preceding diagonal in a scalar; one row suffices for the
        // longest-block recurrence and avoids an allocation per input character.
        let mut row = vec![0; bhi - blo + 1];
        for (i, &ac) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut diagonal = 0;
            for j in blo..bhi {
                let previous = row[j - blo + 1];
                let length = if ac == b[j] { diagonal + 1 } else { 0 };
                row[j - blo + 1] = length;
                diagonal = previous;
                if length > size {
                    (ai, bj, size) = (i + 1 - length, j + 1 - length, length);
                }
            }
        }
        total += size;
        if size > 0 {
            if alo < ai && blo < bj {
                todo.push((alo, ai, blo, bj));
            }
            if ai + size < ahi && bj + size < bhi {
                todo.push((ai + size, ahi, bj + size, bhi));
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command_matcher() -> Matcher {
        Matcher {
            phrases: ["launch deadlock", "launch discord"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            apps: HashSet::new(),
            verbs: vec![],
            prefixes: vec![],
            suffixes: vec![],
            exact_only: false,
            rules: Default::default(),
            blocked_apps: Default::default(),
            candidates: vec![
                ("launch deadlock".into(), "deadlock".into()),
                ("launch discord".into(), "discord".into()),
            ],
        }
    }
    #[test]
    fn disagreement_and_low_confidence_do_not_dispatch() {
        let m = command_matcher();
        assert!(m
            .choose(
                "launch deadlock",
                Some(0.95),
                "launch this cord",
                Some(("launch discord".into(), 0.85))
            )
            .is_none());
        assert!(m.choose("launch discord", Some(0.4), "umm", None).is_none());
        assert!(m.choose("launch [unk]", Some(1.0), "", None).is_none());
        let result = m
            .choose(
                "launch deadlock",
                Some(0.95),
                "launch discord",
                Some(("launch discord".into(), 1.0)),
            )
            .unwrap();
        assert_eq!(result.text, "launch discord");
        assert!(result.closest_match);
    }
    #[test]
    fn agreeing_passes_boost_score_without_losing_word_confidence() {
        let result = command_matcher()
            .choose(
                "launch discord",
                Some(0.7),
                "launch discord",
                Some(("launch discord".into(), 1.0)),
            )
            .unwrap();
        assert_eq!(result.confidence, Some(1.0));
        assert_eq!(result.min_word_posterior, Some(0.7));
        assert!(!result.closest_match);
    }
    #[test]
    fn python_sequence_matcher_tie_breaks() {
        assert_eq!(
            matching_size(
                &"tide".chars().collect::<Vec<_>>(),
                &"diet".chars().collect::<Vec<_>>()
            ),
            1
        );
        assert_eq!(
            matching_size(
                &"diet".chars().collect::<Vec<_>>(),
                &"tide".chars().collect::<Vec<_>>()
            ),
            2
        );
    }
    #[test]
    fn ambiguous_commands_and_partial_words_are_rejected() {
        let m = Matcher {
            phrases: HashSet::new(),
            apps: HashSet::new(),
            verbs: vec![],
            prefixes: vec![],
            suffixes: vec![],
            exact_only: false,
            rules: Default::default(),
            blocked_apps: Default::default(),
            candidates: vec![
                ("open steam".into(), "steam".into()),
                ("open stream".into(), "stream".into()),
                ("end recording".into(), "stop".into()),
            ],
        };
        assert_eq!(m.closest("open steam"), Some(("open steam".into(), 1.0)));
        assert_eq!(m.closest("recording"), None);
    }
}

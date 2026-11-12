//! Notes pinned to agent tabs, their reminders and the `note` palette mode, free of host calls so
//! it tests natively.

use serde::{Deserialize, Serialize};

pub const TEXT_MAX: usize = 120;
pub const MAX_NOTES: usize = 256;
const MIN_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MIN_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// The tab the note is pinned to; surface ids are new after a restart, so it is matched by name then.
    pub surface: String,
    pub name: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remind_at: Option<u64>,
    #[serde(default)]
    pub reminded: bool,
}

/// What a `note` query asks for: `<tab> <text>`, optionally ending in `in 10m`, `in 1h`, `in 2d`.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub target: String,
    pub text: String,
    pub remind_in: Option<u64>,
}

/// Untrusted text on one line, without control or format characters, and short.
pub fn clean(s: &str, max: usize) -> String {
    let one: String = s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !c.is_control() && !is_format(*c)).collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max { one } else { format!("{}…", one.chars().take(max - 1).collect::<String>()) }
}

fn is_format(c: char) -> bool {
    matches!(c, '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{115f}' | '\u{1160}' | '\u{17b4}' | '\u{17b5}' | '\u{180b}'..='\u{180f}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{3164}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{ffa0}' | '\u{fff0}'..='\u{fffb}' | '\u{e0000}'..='\u{e0fff}')
}

/// `10m`, `1h`, `90s` is refused, `2d`; also `10 min`, `1 hour`, `2 days`.
pub fn parse_duration(n: &str, unit: &str) -> Option<u64> {
    let n: u64 = n.parse().ok().filter(|n| *n > 0 && *n <= 999)?;
    let unit_ms = match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => MIN_MS,
        "h" | "hr" | "hrs" | "hour" | "hours" => HOUR_MS,
        "d" | "day" | "days" => DAY_MS,
        _ => return None,
    };
    Some(n * unit_ms)
}

fn split_number(word: &str) -> Option<(&str, &str)> {
    let at = word.find(|c: char| !c.is_ascii_digit())?;
    (at > 0).then(|| word.split_at(at))
}

pub fn parse(query: &str) -> Option<Draft> {
    let words: Vec<&str> = query.split_whitespace().collect();
    let (target, mut rest) = words.split_first()?;
    let mut remind_in = None;
    if let [head @ .., "in", w] = rest
        && let Some(ms) = split_number(w).and_then(|(n, u)| parse_duration(n, u))
    {
        (remind_in, rest) = (Some(ms), head);
    } else if let [head @ .., "in", n, u] = rest
        && let Some(ms) = parse_duration(n, u)
    {
        (remind_in, rest) = (Some(ms), head);
    }
    let text = clean(&rest.join(" "), TEXT_MAX);
    (!text.is_empty()).then(|| Draft { target: clean(target, 64), text, remind_in })
}

/// `45s` under a minute, then `10m`, `1h`, `1h30m`, `2d`.
pub fn short(ms: u64) -> String {
    let m = ms.div_ceil(MIN_MS);
    match m {
        0 => "now".into(),
        1..60 => format!("{m}m"),
        60..1440 if m.is_multiple_of(60) => format!("{}h", m / 60),
        60..1440 => format!("{}h{}m", m / 60, m % 60),
        _ => format!("{}d", m.div_ceil(1440)),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notes {
    pub notes: Vec<Note>,
}

impl Notes {
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// One note per tab: pinning again replaces it.
    pub fn pin(&mut self, surface: &str, name: &str, draft: &Draft, now: u64) {
        self.notes.retain(|n| n.surface != surface);
        self.notes.push(Note {
            surface: surface.into(),
            name: name.into(),
            text: draft.text.clone(),
            remind_at: draft.remind_in.map(|d| now + d),
            reminded: false,
        });
        if self.notes.len() > MAX_NOTES {
            self.notes.remove(0);
        }
    }

    pub fn remove(&mut self, surface: &str) -> bool {
        let before = self.notes.len();
        self.notes.retain(|n| n.surface != surface);
        self.notes.len() != before
    }

    pub fn get(&self, surface: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.surface == surface)
    }

    /// After a restart surface ids are new: a note whose tab is gone moves to the first tab with its name.
    pub fn adopt(&mut self, surface: &str, name: &str, live: impl Fn(&str) -> bool) -> bool {
        if self.get(surface).is_some() {
            return false;
        }
        match self.notes.iter_mut().find(|n| n.name == name && !live(&n.surface)) {
            Some(n) => {
                n.surface = surface.into();
                true
            }
            None => false,
        }
    }

    /// Notes whose reminder is due, marked so each fires once.
    pub fn due(&mut self, now: u64) -> Vec<Note> {
        let mut out = Vec::new();
        for n in &mut self.notes {
            if !n.reminded && n.remind_at.is_some_and(|t| t <= now) {
                n.reminded = true;
                out.push(n.clone());
            }
        }
        out
    }
}

impl Note {
    /// The row's right-hand label, `remind 10m`, only while a reminder is pending.
    pub fn label(&self, now: u64) -> Option<String> {
        let at = self.remind_at.filter(|at| !self.reminded && *at > now)?;
        Some(format!("remind {}", short(at - now)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(q: &str) -> Draft {
        parse(q).unwrap()
    }

    #[test]
    fn a_query_is_a_tab_a_note_and_maybe_a_reminder() {
        assert_eq!(
            draft("api waiting on Konrad's answer in 1h"),
            Draft { target: "api".into(), text: "waiting on Konrad's answer".into(), remind_in: Some(HOUR_MS) }
        );
        assert_eq!(draft("web check the redirect before merging").remind_in, None);
        assert_eq!(draft("web ship it in 10 minutes").remind_in, Some(10 * MIN_MS));
        assert_eq!(draft("web ship it in 2d").remind_in, Some(2 * DAY_MS));
        assert_eq!(draft("web log in in 5m").text, "log in", "only the last `in` is the reminder");
        assert_eq!(draft("web come back in time").text, "come back in time");
        assert_eq!(draft("web wait in 0m").remind_in, None, "a zero reminder is just text");
        assert_eq!(parse("web"), None);
        assert_eq!(parse("web in 5m"), None, "a reminder needs a note");
        assert_eq!(parse("   "), None);
    }

    #[test]
    fn note_text_is_cleaned() {
        assert_eq!(draft("web a\u{202e}b\tc\u{200b}d").text, "ab cd");
        assert_eq!(draft(&format!("web {}", "x".repeat(200))).text.chars().count(), TEXT_MAX);
    }

    #[test]
    fn durations_read_short() {
        assert_eq!([0, 1, 59_000, 10 * MIN_MS, HOUR_MS, 90 * MIN_MS, DAY_MS, 3 * DAY_MS - 1].map(short), ["now", "1m", "1m", "10m", "1h", "1h30m", "1d", "3d"]);
    }

    #[test]
    fn pinning_replaces_and_reminders_fire_once() {
        let mut n = Notes::default();
        n.pin("s1", "web", &draft("web old"), 0);
        n.pin("s1", "web", &draft("web check the redirect in 10m"), 1000);
        assert_eq!(n.notes.len(), 1);
        let note = n.get("s1").unwrap();
        assert_eq!((note.text.as_str(), note.label(1000).as_deref()), ("check the redirect", Some("remind 10m")));
        assert!(n.due(1000 + 9 * MIN_MS).is_empty());
        assert_eq!(n.due(1000 + 10 * MIN_MS).len(), 1);
        assert!(n.due(1000 + 11 * MIN_MS).is_empty(), "fires once");
        assert_eq!(n.get("s1").unwrap().label(0), None, "the label goes once the reminder fired");
        assert!(n.remove("s1") && !n.remove("s1"));
    }

    #[test]
    fn a_note_without_a_reminder_has_no_label() {
        let mut n = Notes::default();
        n.pin("s1", "web", &draft("web hi"), 0);
        assert_eq!(n.get("s1").unwrap().label(0), None);
        assert!(n.due(u64::MAX).is_empty());
    }

    #[test]
    fn notes_move_to_a_new_tab_of_the_same_name_only_when_theirs_is_gone() {
        let mut n = Notes::default();
        n.pin("old", "web", &draft("web hi"), 0);
        assert!(!n.adopt("new", "web", |s| s == "old"), "its tab is still open");
        assert!(!n.adopt("new", "api", |_| false));
        assert!(n.adopt("new", "web", |_| false));
        assert_eq!(n.get("new").unwrap().text, "hi");
    }

    #[test]
    fn notes_are_capped_and_round_trip() {
        let mut n = Notes::default();
        for i in 0..=MAX_NOTES {
            n.pin(&format!("s{i}"), "x", &draft("x y"), 0);
        }
        assert_eq!(n.notes.len(), MAX_NOTES);
        assert!(n.get("s0").is_none());
        assert_eq!(Notes::parse(&n.to_json()).unwrap(), n);
    }
}

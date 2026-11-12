//! Sticky notes: the ⌘K `note` mode pins a note to an agent tab, shown on its row, with an optional
//! reminder. Notes live in kv.
// wit-bindgen's flattened `notify` import has 14 params.
#![allow(clippy::too_many_arguments)]

wit_bindgen::generate!({ path: "wit/haunt-plugin.wit", world: "plugin" });

mod notes;

use std::collections::BTreeMap;
use std::sync::Mutex;

use haunt::plugin::log::log;
use haunt::plugin::rows::{self, Decoration, Tone};
use haunt::plugin::types::LogLevel;
use haunt::plugin::{kv, notify, palette, timer};
use notes::{Draft, Notes, clean, parse, short};
use serde::Deserialize;

pub const MODE: &str = "note";
const KEY: &str = "notes";
const TICK_SECS: u32 = 15;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Show the note on its tab's row.
    pub row: bool,
    /// Notify when a reminder is due.
    pub reminders: bool,
    /// Learn tab names from events without the tab's nonce, which anything in the terminal can forge.
    pub unverified: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { row: true, reminders: true, unverified: false }
    }
}

#[derive(Default)]
struct Live {
    settings: Settings,
    /// Tabs seen this run: surface → row name.
    tabs: BTreeMap<String, String>,
    draft: Option<Draft>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Live) -> R) -> R {
    f(LIVE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Live::default))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn load() -> Notes {
    kv::get(KEY)
        .map(|j| {
            Notes::parse(&j).unwrap_or_else(|e| {
                log(LogLevel::Warn, &format!("sticky-notes: saved notes unreadable ({e}); starting fresh"));
                Notes::default()
            })
        })
        .unwrap_or_default()
}

fn save(n: &Notes) {
    if let Err(e) = kv::set(KEY, &n.to_json()) {
        log(LogLevel::Warn, &format!("sticky-notes: notes not saved: {e}"));
    }
}

/// The row's decoration for a note: `remind 10m` on the right, `✎ text` after the meta line.
pub fn decoration(note: &notes::Note, now: u64) -> Decoration {
    Decoration {
        label: note.label(now),
        label_tone: Tone::Dim,
        suffix: Some(format!("✎ {}", note.text)),
        suffix_tone: Tone::Text,
        scroll: false,
        bar: None,
        spark: None,
    }
}

fn redraw(n: &Notes, live: &Live, now: u64) {
    for note in &n.notes {
        if !live.tabs.contains_key(&note.surface) {
            continue;
        }
        let d = live.settings.row.then(|| decoration(note, now));
        if let Err(e) = rows::decorate(&note.surface, d.as_ref()) {
            log(LogLevel::Debug, &format!("sticky-notes: {}: {e}", note.surface));
        }
    }
}

/// The palette rows for a query: a pin row for a draft, else the notes there are.
pub fn items(query: &str, n: &Notes, tabs: &BTreeMap<String, String>, now: u64) -> (Vec<PaletteItem>, Option<Draft>) {
    let item = |id: String, title: String, subtitle: Option<String>, surface: Option<String>| PaletteItem { id, title, subtitle, at_ms: None, mark: None, surface };
    if let Some(d) = parse(query) {
        let target = tabs.iter().find(|(_, name)| name.eq_ignore_ascii_case(&d.target));
        return match target {
            Some((surface, name)) => {
                let title = match d.remind_in {
                    Some(ms) => format!("Pin to {name}, remind in {}", short(ms)),
                    None => format!("Pin to {name}"),
                };
                (vec![item(format!("pin:{surface}"), title, Some("↵".into()), None)], Some(d))
            }
            None => (vec![item("none".into(), format!("No agent tab named {}", d.target), None, None)], None),
        };
    }
    let q = query.trim().to_lowercase();
    let list = n
        .notes
        .iter()
        .filter(|note| tabs.contains_key(&note.surface))
        .filter(|note| q.is_empty() || note.name.to_lowercase().starts_with(&q) || note.text.to_lowercase().contains(&q))
        .map(|note| item(format!("open:{}", note.surface), format!("{}: {}", note.name, note.text), note.label(now), Some(note.surface.clone())))
        .collect();
    (list, None)
}

fn name_of(cwd: Option<&str>) -> Option<String> {
    cwd.and_then(|c| c.trim_end_matches('/').rsplit('/').next()).map(|n| clean(n, 48)).filter(|n| !n.is_empty())
}

struct Plugin;

impl Guest for Plugin {
    fn init(json: String) {
        with(|l| l.settings = serde_json::from_str(&json).unwrap_or_default());
        palette::mode(MODE, "Pin a note to an agent tab");
        if let Err(e) = timer::every(TICK_SECS) {
            log(LogLevel::Warn, &format!("sticky-notes: no timer: {e}"));
        }
    }

    fn on_settings_changed(json: String) {
        let n = load();
        with(|l| {
            l.settings = serde_json::from_str(&json).unwrap_or_default();
            redraw(&n, l, now_ms());
        });
    }

    fn on_event(event: AgentEvent) {
        let (Some(surface), Some(name)) = (event.surface.as_deref(), name_of(event.cwd.as_deref())) else { return };
        with(|l| {
            if (!event.verified && !l.settings.unverified) || l.tabs.get(surface) == Some(&name) {
                return;
            }
            l.tabs.insert(surface.to_owned(), name.clone());
            let mut n = load();
            if n.adopt(surface, &name, |s| l.tabs.contains_key(s)) {
                save(&n);
            }
            redraw(&n, l, now_ms());
        });
    }

    fn palette_items(mode: String, query: String) -> Vec<PaletteItem> {
        if mode != MODE {
            return Vec::new();
        }
        let n = load();
        with(|l| {
            let (list, draft) = items(&query, &n, &l.tabs, now_ms());
            l.draft = draft;
            list
        })
    }

    fn on_action(action_id: String, payload: String) {
        let now = now_ms();
        if action_id == "remove" {
            let mut n = load();
            if n.remove(&payload) {
                save(&n);
                let _ = rows::decorate(&payload, None);
            }
            return;
        }
        let Some(surface) = action_id.strip_prefix("pin:") else { return };
        with(|l| {
            let (Some(d), Some(name)) = (l.draft.take(), l.tabs.get(surface).cloned()) else { return };
            let mut n = load();
            n.pin(surface, &name, &d, now);
            save(&n);
            redraw(&n, l, now);
        });
    }

    fn row_menu(surface: String) -> Vec<MenuItem> {
        if load().get(&surface).is_none() {
            return Vec::new();
        }
        vec![MenuItem { id: "remove".into(), title: "Remove note".into(), checked: false, parent: None }]
    }

    fn on_row_moved(_surface: String, _group: String, _before: Option<String>) {}

    fn on_theme_changed() {}

    fn on_tick(now_ms: u64) {
        let mut n = load();
        let due = n.due(now_ms);
        if !due.is_empty() {
            save(&n);
        }
        with(|l| {
            if l.settings.reminders {
                for note in due.iter().filter(|note| l.tabs.contains_key(&note.surface)) {
                    let open = notify::Action { id: format!("open:{}", note.surface), title: "Open agent".into() };
                    if let Err(e) = notify::notify(&format!("{} · reminder", note.name), &note.text, &[open], Some(&note.surface)) {
                        log(LogLevel::Warn, &format!("sticky-notes: reminder not shown: {e}"));
                    }
                }
            }
            redraw(&n, l, now_ms);
        });
    }

    fn on_exec(_job: u64, _outcome: Result<haunt::plugin::exec::Output, String>) {}

    fn on_away_changed(_away: bool, _away_ms: u64) {}

    fn on_hover_hold(_held: bool, _surface: String) {}

    fn on_fs_tree(_job: u64, _outcome: Result<haunt::plugin::fs_list::TreeResult, String>) {}
}

export!(Plugin);

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs() -> BTreeMap<String, String> {
        BTreeMap::from([("s-api".into(), "api".into()), ("s-web".into(), "web".into())])
    }

    #[test]
    fn a_draft_offers_one_pin_row_like_the_mockup() {
        let (list, d) = items("api waiting on Konrad's answer in 1h", &Notes::default(), &tabs(), 0);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].id.as_str(), list[0].title.as_str(), list[0].subtitle.as_deref()), ("pin:s-api", "Pin to api, remind in 1h", Some("↵")));
        assert_eq!(d.unwrap().text, "waiting on Konrad's answer");
        assert_eq!(items("API hi", &Notes::default(), &tabs(), 0).0[0].title, "Pin to api", "names match in any case");
    }

    #[test]
    fn an_unknown_tab_is_said_and_pins_nothing() {
        let (list, d) = items("shop hi", &Notes::default(), &tabs(), 0);
        assert_eq!((list[0].id.as_str(), list[0].title.as_str(), d), ("none", "No agent tab named shop", None));
    }

    #[test]
    fn without_a_draft_the_open_tabs_notes_are_listed() {
        let mut n = Notes::default();
        n.pin("s-web", "web", &parse("web check the redirect before merging in 10m").unwrap(), 0);
        n.pin("gone", "old", &parse("old bye").unwrap(), 0);
        let (list, _) = items("", &n, &tabs(), 0);
        assert_eq!(list.len(), 1, "a note whose tab is closed waits for a tab of its name");
        assert_eq!((list[0].title.as_str(), list[0].subtitle.as_deref(), list[0].surface.as_deref()), ("web: check the redirect before merging", Some("remind 10m"), Some("s-web")));
        assert_eq!(items("we", &n, &tabs(), 0).0.len(), 1);
        assert!(items("zz", &n, &tabs(), 0).0.is_empty());
    }

    #[test]
    fn the_row_shows_the_reminder_and_the_note() {
        let mut n = Notes::default();
        n.pin("s", "web", &parse("web check the redirect before merging in 10m").unwrap(), 0);
        let d = decoration(n.get("s").unwrap(), 0);
        assert_eq!((d.label.as_deref(), d.suffix.as_deref()), (Some("remind 10m"), Some("✎ check the redirect before merging")));
        assert!(matches!((d.label_tone, d.suffix_tone), (Tone::Dim, Tone::Text)));
        n.due(10 * 60_000);
        assert_eq!(decoration(n.get("s").unwrap(), 10 * 60_000).label, None);
    }

    #[test]
    fn names_come_from_the_cwd() {
        assert_eq!(name_of(Some("/w/web/")).as_deref(), Some("web"));
        assert_eq!(name_of(Some("/")), None);
    }

    #[test]
    fn manifest_asks_for_what_it_uses_and_defaults_match() {
        let m = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/haunt-plugin.toml")).unwrap();
        let t: toml::Table = toml::from_str(&m).unwrap();
        let caps = t["capabilities"].as_table().unwrap();
        assert_eq!(caps.keys().collect::<Vec<_>>(), ["decorate", "events", "notifications", "timer"]);
        let defaults: serde_json::Map<String, serde_json::Value> =
            t["settings"].as_table().unwrap().iter().map(|(k, v)| (k.clone(), serde_json::to_value(&v["default"]).unwrap())).collect();
        assert_eq!(serde_json::from_value::<Settings>(serde_json::Value::Object(defaults)).unwrap(), Settings::default());
    }
}

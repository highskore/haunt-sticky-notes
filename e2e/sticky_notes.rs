//! End to end against a real haunt core. Copy into haunt's `cli/tests/` and run
//! `cargo test -p haunt-cli --features plugins --test sticky_notes` (see README).
#![cfg(feature = "plugins")]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use haunt_core::plugin_settings::PluginConfig;
use haunt_core::plugin_store::{Store, attach};
use haunt_core::plugins::PluginHost;
use haunt_core::{Core, SurfaceEnv};

const ID: &str = "dev.haunt.sticky-notes";

fn repo() -> PathBuf {
    std::env::var_os("HAUNT_PLUGINS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../haunt-plugins"))
        .join("haunt-sticky-notes")
}

fn until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn send(sock: &mut UnixStream, env: &SurfaceEnv, nonce: bool, cwd: &str, event: &str) {
    let nonce = if nonce { format!(r#","nonce":"{}""#, env.nonce) } else { String::new() };
    let line = format!(r#"{{"v":1,"agent":"claude","surface_id":"{}"{nonce},"session_id":"s1","cwd":"{cwd}","event":"{event}"}}"#, env.surface_id);
    sock.write_all(format!("{line}\n").as_bytes()).unwrap();
}

#[test]
fn a_note_pinned_from_the_palette_shows_on_the_row() {
    let dir = repo();
    if !dir.join("haunt-plugin.toml").is_file() {
        eprintln!("skipped: haunt-sticky-notes not found; set HAUNT_PLUGINS_DIR");
        return;
    }
    let home = PathBuf::from(format!("/tmp/haunt-fp-notes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .unwrap();
    assert!(status.success());
    let o = Command::new(env!("CARGO_BIN_EXE_haunt")).args(["plugin", "install", dir.to_str().unwrap(), "--yes"]).env("HAUNT_HOME", &home).output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));

    let core = Core::new(&home);
    core.start().unwrap();
    let (web, api) = (core.register_surface("/tmp").unwrap(), core.register_surface("/tmp").unwrap());
    let set = Store::open(&home).unwrap().load_all(&PluginHost::new().unwrap(), Some(&core.handle())).unwrap();
    assert!(set.errors().is_empty(), "{:?}", set.errors());
    let set = Arc::new(Mutex::new(set));
    attach(&core, &set);
    let items = |q: &str| set.lock().unwrap().palette_items(Some("note"), q).into_iter().map(|i| (i.id, i.title, i.subtitle)).collect::<Vec<_>>();
    let decoration = |s: &str| core.snapshot().groups.iter().flat_map(|g| &g.rows).find(|r| r.surface_id == s).and_then(|r| r.decoration.clone());

    let mut sock = UnixStream::connect(&web.socket_path).unwrap();
    send(&mut sock, &web, true, "/tmp/web", "session_start");
    send(&mut sock, &api, false, "/tmp/api", "session_start");
    until("the web tab to be known", || items("web hi")[0].1 == "Pin to web");
    assert_eq!(items("api hi")[0].1, "No agent tab named api", "a forged event names no tab");

    let pin = items("web check the redirect before merging in 10m");
    assert_eq!(pin[0].1, "Pin to web, remind in 10m");
    set.lock().unwrap().invoke(ID, &pin[0].0, true).unwrap();
    let d = decoration(&web.surface_id).expect("the row is decorated");
    assert_eq!((d.label.as_deref(), d.suffix.as_deref()), (Some("remind 10m"), Some("✎ check the redirect before merging")));
    assert_eq!(items("")[0].1, "web: check the redirect before merging");

    set.lock().unwrap().set_config(&PluginConfig::parse(&format!("[plugins.\"{ID}\"]\nrow = false\n")));
    assert_eq!(decoration(&web.surface_id), None, "the row setting takes it off");
    set.lock().unwrap().set_config(&PluginConfig::parse(""));
    assert!(decoration(&web.surface_id).is_some());

    let menu = set.lock().unwrap().row_menu(&web.surface_id);
    assert_eq!(menu.iter().map(|m| (m.plugin_id.as_str(), m.id.as_str(), m.title.as_str())).collect::<Vec<_>>(), [(ID, "remove", "Remove note")]);
    assert!(set.lock().unwrap().row_menu(&api.surface_id).is_empty());
    set.lock().unwrap().invoke_row_item(ID, "remove", &web.surface_id).unwrap();
    assert_eq!(decoration(&web.surface_id), None);
    assert!(items("").is_empty());

    drop(core);
    let _ = std::fs::remove_dir_all(&home);
}

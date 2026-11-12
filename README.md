# Sticky notes

Pin a note to any agent tab in Haunt, with an optional reminder.

Type `note` in the ⌘K palette, then the tab's name, the note and, if you want a reminder, `in 10m`, `in 1h` or `in 2d`:

```
note api waiting on Konrad's answer in 1h      →  Pin to api, remind in 1h
```

The note shows on the tab's row after its meta line (`feat/login · ✎ check the redirect before merging`), and while a reminder is pending the right-hand slot counts down (`remind 10m`). When it's due Haunt sends a notification and the countdown goes. One note per tab: pinning again replaces it. Remove it from the row's context menu (**Remove note**). `note` with no text lists the notes on open tabs; Enter jumps to the tab.

Tab names are the last part of the agent's folder, as the sidebar shows them. Notes are kept in the plugin's own storage. Surface ids are new after a restart, so a note whose tab is gone moves to the next tab with the same name.

## Settings

In `config.toml`:

```toml
[plugins."dev.haunt.sticky-notes"]
row = true          # show the note and its reminder on the row
reminders = true    # notify when a reminder is due
unverified = false  # also learn tab names from events without the tab's nonce
```

## Capabilities

| capability | why |
|---|---|
| `events = ["agent.session-start", "agent.stop"]` | to learn which tab runs which agent; no event text is kept |
| `decorate` | the note and countdown on the row |
| `notifications` | the reminder |
| `timer` | checks reminders every 15 seconds |

Events without the tab's nonce are ignored unless `unverified` is on, since anything printed in the terminal can forge them.

## Build and test

```sh
cargo build --release --target wasm32-wasip2   # or ./build.sh
cargo test                                     # unit tests
haunt plugin install .
```

`e2e/sticky_notes.rs` runs the built plugin against a real Haunt core: copy it into a Haunt checkout's `cli/tests/` and run `HAUNT_PLUGINS_DIR=<this repo's parent> cargo test -p haunt-cli --test sticky_notes`.

Needs Haunt with plugin API 0.5 (`api = "0.5"`).

## Licence

MIT, see `LICENSE`.

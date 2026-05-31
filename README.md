# helicopter

A GNOME desktop helicopter reminder animation.

Helicopter runs in the background with a top-bar status icon, reads upcoming Google Calendar events, and shows a transparent always-on-top helicopter banner animation using the event title.

## Quick start

```sh
cargo build --release
./target/release/helicopter
```

## Setup

- Google Calendar: `docs/google-calendar.md`
- GNOME Shell: you may want `gnome-shell-extension-appindicator` for tray/status icons (see `Cargo.toml` deb metadata).


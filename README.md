# hyper-sync

Low-latency LED sync daemon for Skydimo strips on Linux/Wayland. Drives the strip over USB serial (CH340) using the Skydimo `Ada` protocol and can mirror screen edges via the xdg-desktop-portal screencast + PipeWire.

## Requirements

- Linux with Wayland (KDE Plasma tested)
- Rust toolchain
- For **screen** mode: `pipewire-devel`, `gcc` (bindgen), portal screencast permission
- Skydimo controller on USB serial (default `/dev/ttyUSB0`, 115200 baud)

## Build

```bash
./scripts/build.sh
```

Builds with all features (daemon, TUI, tray, screen). Installs to `~/.cargo/bin/hyper-sync`, `~/.local/bin/hyper-sync`, the tray icon at `~/.local/share/icons/hyper-sync/hyper-hdr.png`, and a KDE autostart entry at `~/.config/autostart/hyper-sync.desktop`.

On Fedora, `scripts/build.sh` sets `BINDGEN_EXTRA_CLANG_ARGS` for system headers. Adjust the gcc include path if your toolchain differs.

Build without screen capture (solid/candle/off + daemon only):

```bash
HYPER_SYNC_FEATURES=daemon cargo build --release
```

## Daemon + TUI + tray (recommended)

One background process owns the serial port and active effect. TUI and tray are thin clients over a Unix socket.

```bash
# Start daemon (tray icon appears on KDE)
hyper-sync daemon

# Headless daemon (no tray)
hyper-sync daemon --no-tray

# Interactive control panel — q detaches, daemon keeps running
hyper-sync tui

# CLI control
hyper-sync ctl status
hyper-sync ctl status --json
hyper-sync ctl stop          # effect off
hyper-sync ctl restart
hyper-sync ctl quit          # shut down daemon
hyper-sync ctl set --mode screen --brightness 0.2
```

Shared config: `~/.config/hyper-sync/config.toml` (created on first daemon start). Edit the file or use TUI/tray — changes apply instantly via inotify reload.

Example config is in `config/runtime.toml`.

IPC socket: `$XDG_RUNTIME_DIR/hyper-sync.sock` (fallback `~/.cache/hyper-sync/hyper-sync.sock`).

### Login autostart

`./scripts/build.sh` installs `~/.config/autostart/hyper-sync.desktop` (KDE autostart after panel).

Alternatively, systemd user unit:

```bash
cp systemd/hyper-sync.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now hyper-sync.service
```

### Tray

Right-click the tray icon for start/stop/restart, effect mode, brightness, color presets, open TUI, and quit daemon. Left-click toggles off ↔ last effect.

## Direct CLI (legacy shortcuts)

These still work standalone. If the daemon is already running, they patch config via IPC instead:

```bash
# Screen edge sync — portal dialog on first run only
hyper-sync screen --port /dev/ttyUSB0 --leds 65 --fps 30 --brightness 0.8

# Re-pick monitor / re-grant permission
hyper-sync screen --port /dev/ttyUSB0 --leds 65 --forget-portal

# Solid color
hyper-sync solid --port /dev/ttyUSB0 --leds 65 --color ff0000 --brightness 0.8

# Candle effect
hyper-sync candle --port /dev/ttyUSB0 --leds 65

# All off
hyper-sync off --port /dev/ttyUSB0 --leds 65
```

### Screen capture permission

On first run, the portal asks which monitor to share. Allow persistence if prompted — hyper-sync saves a restore token to `~/.config/hyper-sync/restore-token` and reuses it on later runs (no dialog).

- Re-pick a monitor: `--forget-portal`
- Revoke in system settings (KDE: Settings → Privacy → Screen Sharing)

### Layout

LED mapping is defined in `config/layout.toml` (default: 65 LEDs, U-shape, origin bottom-right, 17 right + 31 top + 17 left). Override in config or with `--layout path/to/layout.toml`.

## Protocol

Skydimo frame: `Ada` + `0x00 0x00` + raw LED count + RGB×N (no Adalight checksum).

## License

MIT

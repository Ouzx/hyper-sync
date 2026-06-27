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

Installs to `~/.cargo/bin/hyper-sync` and `~/.local/bin/hyper-sync`.

On Fedora, `scripts/build.sh` sets `BINDGEN_EXTRA_CLANG_ARGS` for system headers. Adjust the gcc include path if your toolchain differs.

Build without screen capture (solid/candle/off only):

```bash
cargo build --release
```



## Usage

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

LED mapping is defined in `config/layout.toml` (default: 65 LEDs, U-shape, origin bottom-right, 17 right + 31 top + 17 left). Override with `--layout path/to/layout.toml`.

### systemd (optional)

```bash
cp systemd/hyper-sync.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now hyper-sync.service
```

Edit the unit file for your serial port, brightness, and NVIDIA offload env vars if needed.

## Protocol

Skydimo frame: `Ada` + `0x00 0x00` + raw LED count + RGB×N (no Adalight checksum).

## License

MIT
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="${HOME}/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:${PATH}"
export CARGO_TARGET_DIR="${PWD}/target"
export BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-redhat-linux/16/include -isystem /usr/include"

FEATURES="${HYPER_SYNC_FEATURES:-full}"
# One build, pinned to Cargo.lock (`cargo install` re-resolved every dependency and built again).
cargo build --release --locked --features "${FEATURES}" "$@"
install -Dm755 target/release/hyper-sync "${HOME}/.cargo/bin/hyper-sync"
install -Dm755 target/release/hyper-sync "${HOME}/.local/bin/hyper-sync"

# App icon for the menu / autostart entries (Icon=hyper-sync); the tray has it built in.
install -Dm644 assets/hyper-sync.svg "${HOME}/.local/share/icons/hicolor/scalable/apps/hyper-sync.svg"
install -Dm644 assets/hyper-sync.png "${HOME}/.local/share/icons/hicolor/256x256/apps/hyper-sync.png"

# KDE Plasma: the System Tray widget (plasmashell picks up changes to an installed one after a restart).
if command -v kpackagetool6 >/dev/null; then
  kpackagetool6 -t Plasma/Applet -u plasmoid >/dev/null 2>&1 || kpackagetool6 -t Plasma/Applet -i plasmoid
fi

#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="${HOME}/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:${PATH}"
export CARGO_TARGET_DIR="${PWD}/target"
export BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-redhat-linux/16/include -isystem /usr/include"

FEATURES="${HYPER_SYNC_FEATURES:-full}"
cargo build --release --features "${FEATURES}" "$@"
cargo install --path . --force --features "${FEATURES}"
install -Dm755 target/release/hyper-sync "${HOME}/.local/bin/hyper-sync"

if [[ -f assets/hyper-hdr.png ]]; then
  install -Dm644 assets/hyper-hdr.png "${HOME}/.local/share/icons/hyper-sync/hyper-hdr.png"
fi

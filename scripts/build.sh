#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="${HOME}/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:${PATH}"
export CARGO_TARGET_DIR="${PWD}/target"
export BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-redhat-linux/16/include -isystem /usr/include"
cargo build --release --features screen "$@"
cargo install --path . --force --features screen
install -Dm755 target/release/hyper-sync "${HOME}/.local/bin/hyper-sync"

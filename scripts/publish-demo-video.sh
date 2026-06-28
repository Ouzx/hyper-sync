#!/usr/bin/env bash
# GitHub README strips <video src="assets/..."> and raw.githubusercontent.com.
# Inline playback needs a release asset URL or a user-attachments URL from drag-drop.
set -euo pipefail
cd "$(dirname "$0")/.."

TAG=demo
ASSET=assets/untitled.mp4
URL="https://github.com/Ouzx/hyper-sync/releases/download/${TAG}/untitled.mp4"

gh release delete "$TAG" -y 2>/dev/null || true
gh release create "$TAG" "$ASSET" \
  --title "README demo video" \
  --notes "MP4 embedded in README. Re-run ./scripts/publish-demo-video.sh after replacing assets/untitled.mp4."

echo "Published: $URL"
echo "README should already point at this URL."

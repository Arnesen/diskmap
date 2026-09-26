#!/bin/sh
# Build and install diskmap for the current user.
set -eu
cd "$(dirname "$0")"
cargo build --release
install -Dm755 target/release/diskmap "$HOME/.local/bin/diskmap"
install -Dm644 data/diskmap.desktop "$HOME/.local/share/applications/dev.txcb.DiskMap.desktop"
echo "Installed ~/.local/bin/diskmap and the Disk Map launcher entry."
command -v compsize >/dev/null || echo "Tip: sudo pacman -S compsize  (enables 'Measure real size')"

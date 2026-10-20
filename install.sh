#!/bin/bash
# Builds Joule and installs it for the current user; safe to run again to update.
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release

install -Dm755 target/release/jouled ~/.local/bin/jouled
install -Dm755 target/release/joule ~/.local/bin/joule
install -Dm644 dist/jouled.service ~/.config/systemd/user/jouled.service
install -Dm644 dist/app.joule.Joule.desktop ~/.local/share/applications/app.joule.Joule.desktop
install -Dm644 dist/app.joule.Joule.svg ~/.local/share/icons/hicolor/scalable/apps/app.joule.Joule.svg

systemctl --user daemon-reload
systemctl --user enable jouled.service
systemctl --user restart jouled.service

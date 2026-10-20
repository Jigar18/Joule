# Joule

Battery and screen-time history for Omarchy. `jouled` records battery level, charging sessions, screen-on time and per-app usage in the background, and `joule` shows it.

## Requirements

Omarchy (Hyprland) on a laptop, plus:

```bash
sudo pacman -S --needed rust gtk4 libadwaita sqlite
```

## Install

```bash
./install.sh
```

The script builds a release binary and installs it for the current user. Run it again after pulling changes to update. It puts these files in place:

| File | Location |
|------|----------|
| Collector | `~/.local/bin/jouled` |
| Window | `~/.local/bin/joule` |
| Systemd user unit (enabled and started) | `~/.config/systemd/user/jouled.service` |
| Launcher entry | `~/.local/share/applications/app.joule.Joule.desktop` |
| Icon | `~/.local/share/icons/hicolor/scalable/apps/app.joule.Joule.svg` |

Open Joule from the Omarchy app launcher, or run `joule`.

## Omarchy setup

Nothing outside this repo needs configuring:

- **Colours:** read from the current Omarchy theme at `~/.local/state/omarchy/current/theme/colors.toml` each time the window opens.
- **Corner radius:** taken from Hyprland's `decoration:rounding`.
- **Window behaviour:** Hyprland floats Joule as a centred card by itself, so no window rule is needed. Super+F, maximise and tiling all work as usual.
- **Collector environment:** `jouled` talks to Hyprland, so it needs `HYPRLAND_INSTANCE_SIGNATURE` in the systemd user environment. Omarchy's session already provides it.

## Data

History is kept in `~/.local/share/joule/joule.db` and pruned to the last 8 days.

## Uninstall

```bash
systemctl --user disable --now jouled.service
```

```bash
rm ~/.local/bin/joule ~/.local/bin/jouled ~/.config/systemd/user/jouled.service ~/.local/share/applications/app.joule.Joule.desktop ~/.local/share/icons/hicolor/scalable/apps/app.joule.Joule.svg
```

```bash
rm -r ~/.local/share/joule
```

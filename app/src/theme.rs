use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

// Fixed rather than taken from the theme: many Omarchy themes repaint "green" and
// "yellow" into unrelated hues, and these two have to stay recognisable.
pub const PLUGGED: Color = Color::from_rgb(0x5f, 0xd0, 0x8a);
pub const ON_BATTERY: Color = Color::from_rgb(0xf3, 0xbd, 0x4e);

impl Color {
    pub const BLACK: Color = Color::from_rgb(0, 0, 0);

    const fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self { red: red as f64 / 255.0, green: green as f64 / 255.0, blue: blue as f64 / 255.0 }
    }

    fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#')?;
        if hex.len() != 6 {
            return None;
        }
        let channel = |index: usize| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok();
        Some(Self::from_rgb(channel(0)?, channel(2)?, channel(4)?))
    }

    fn is_light(self) -> bool {
        0.299 * self.red + 0.587 * self.green + 0.114 * self.blue > 0.6
    }

    fn css(self) -> String {
        let channel = |value: f64| (value * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", channel(self.red), channel(self.green), channel(self.blue))
    }
}

#[derive(Clone)]
pub struct Palette {
    pub dark: bool,
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    /// Corner radius in pixels, shared by every panel, button and drawn shape.
    pub radius: f64,
}

/// Hyprland's window rounding, which Omarchy's own bar and panels follow too.
fn hyprland_corner_rounding() -> Option<f64> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let socket_path = PathBuf::from(runtime_dir).join("hypr").join(instance).join(".socket.sock");

    let mut socket = UnixStream::connect(socket_path).ok()?;
    socket.write_all(b"getoption decoration:rounding").ok()?;
    let mut reply = String::new();
    socket.read_to_string(&mut reply).ok()?;
    reply.lines().find_map(|line| line.strip_prefix("int: "))?.trim().parse().ok()
}

impl Palette {
    /// The colours of the current Omarchy theme, or libadwaita's dark defaults without one.
    pub fn load() -> Self {
        let mut palette = Self {
            dark: true,
            background: Color::from_rgb(0x1d, 0x1d, 0x20),
            foreground: Color::from_rgb(0xff, 0xff, 0xff),
            accent: Color::from_rgb(0x35, 0x84, 0xe4),
            // Large window rounding looks bloated on small controls.
            radius: hyprland_corner_rounding().unwrap_or(0.0).min(12.0),
        };

        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let Ok(theme) = std::fs::read_to_string(home.join(".local/state/omarchy/current/theme/colors.toml")) else {
            return palette;
        };
        for line in theme.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            match (key.trim(), Color::from_hex(value)) {
                ("mode", _) => palette.dark = value != "light",
                ("background", Some(color)) => palette.background = color,
                ("foreground", Some(color)) => palette.foreground = color,
                ("accent", Some(color)) => palette.accent = color,
                _ => {}
            }
        }
        palette
    }

    pub fn apply(&self) {
        let scheme = if self.dark { adw::ColorScheme::ForceDark } else { adw::ColorScheme::ForceLight };
        adw::StyleManager::default().set_color_scheme(scheme);

        let on_accent = if self.accent.is_light() { Color::BLACK } else { Color::from_rgb(0xff, 0xff, 0xff) };
        let css = format!(
            ":root {{
                --window-bg-color: {background};
                --window-fg-color: {foreground};
                --accent-bg-color: {accent};
                --accent-color: {accent};
                --accent-fg-color: {on_accent};
                --plugged-color: {plugged};
                --on-battery-color: {on_battery};
                --radius: {radius}px;
            }}
            {stylesheet}",
            background = self.background.css(),
            foreground = self.foreground.css(),
            accent = self.accent.css(),
            on_accent = on_accent.css(),
            plugged = PLUGGED.css(),
            on_battery = ON_BATTERY.css(),
            radius = self.radius,
            stylesheet = include_str!("style.css"),
        );

        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    }
}

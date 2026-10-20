use gtk::glib;
use std::path::Path;

const FALLBACK_ICON: &str = "application-x-executable";

pub struct AppIdentity {
    pub name: String,
    /// An icon theme name, or an absolute path to an image.
    pub icon: String,
}

struct DesktopEntry {
    id: String,
    executable: Option<String>,
    window_class: Option<String>,
    name: String,
    icon: Option<String>,
    shown_in_launcher: bool,
}

/// The installed applications, used to give tracked processes a proper name and icon.
pub struct DesktopApps {
    entries: Vec<DesktopEntry>,
}

impl DesktopApps {
    pub fn scan() -> Self {
        let mut entries: Vec<DesktopEntry> = Vec::new();
        // The user's directory comes first so its entries override system ones.
        let data_dirs = std::iter::once(glib::user_data_dir()).chain(glib::system_data_dirs());
        for data_dir in data_dirs {
            let Ok(files) = std::fs::read_dir(data_dir.join("applications")) else {
                continue;
            };
            for file in files.flatten() {
                if let Some(entry) = read_desktop_entry(&file.path())
                    && !entries.iter().any(|known| known.id == entry.id)
                {
                    entries.push(entry);
                }
            }
        }
        Self { entries }
    }

    /// `app` is the executable name the collector tracks; `window_class` is the
    /// class of the last window it was seen with, if it ever had one.
    pub fn identify(&self, app: &str, window_class: Option<&str>) -> AppIdentity {
        let app = app.to_lowercase();
        let window_class = window_class.map(str::to_lowercase);
        let has_class = |candidate: &Option<String>| candidate.is_some() && *candidate == window_class;

        let entry = self
            .entries
            .iter()
            .find(|entry| entry.id == app)
            .or_else(|| self.entries.iter().find(|entry| has_class(&Some(entry.id.clone())) || has_class(&entry.window_class)))
            .or_else(|| self.entries.iter().find(|entry| entry.shown_in_launcher && entry.executable.as_deref() == Some(&app)));

        match entry {
            Some(entry) => AppIdentity {
                name: entry.name.clone(),
                icon: entry.icon.clone().unwrap_or_else(|| FALLBACK_ICON.to_owned()),
            },
            None => AppIdentity { name: app, icon: FALLBACK_ICON.to_owned() },
        }
    }
}

fn read_desktop_entry(path: &Path) -> Option<DesktopEntry> {
    const GROUP: &str = "Desktop Entry";

    if path.extension()? != "desktop" {
        return None;
    }
    let file = glib::KeyFile::new();
    file.load_from_file(path, glib::KeyFileFlags::NONE).ok()?;

    let executable = file.string(GROUP, "Exec").ok().and_then(|command| {
        let program = command.split_whitespace().next()?.trim_matches('"');
        Some(Path::new(program).file_name()?.to_string_lossy().to_lowercase())
    });

    Some(DesktopEntry {
        id: path.file_stem()?.to_string_lossy().to_lowercase(),
        executable,
        window_class: file.string(GROUP, "StartupWMClass").ok().map(|class| class.to_lowercase()),
        name: file.locale_string(GROUP, "Name", None).ok()?.into(),
        icon: file.string(GROUP, "Icon").ok().map(Into::into),
        shown_in_launcher: !file.boolean(GROUP, "NoDisplay").unwrap_or(false),
    })
}

mod chart;
mod desktop_apps;
mod format;
mod meter;
mod sparkline;
mod theme;
mod timeline;
mod window;

use adw::prelude::*;
use gtk::glib;

const APP_ID: &str = "app.joule.Joule";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| match app.active_window() {
        // Launching again while open just brings the existing window forward.
        Some(window) => window.present(),
        None => window::build(app).present(),
    });
    app.run()
}

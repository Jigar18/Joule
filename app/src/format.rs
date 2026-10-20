use gtk::glib;

/// "5h 42m", rounded to the nearest minute.
pub fn duration(seconds: i64) -> String {
    let minutes = (seconds + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

/// Formats a moment in local time with a strftime-style pattern.
pub fn local_time(ts: i64, pattern: &str) -> String {
    glib::DateTime::from_unix_local(ts)
        .and_then(|time| time.format(pattern))
        .map(String::from)
        .unwrap_or_default()
}

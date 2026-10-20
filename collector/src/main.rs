mod hyprland;
mod power;
mod processes;

use hyprland::Hyprland;
use joule_store::{AppSeconds, MAX_AWAKE_GAP_SECONDS, RETENTION_SECONDS, Sample, Store};
use power::PowerEvents;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io;
use std::os::fd::RawFd;
use std::time::{SystemTime, UNIX_EPOCH};

const SAMPLE_INTERVAL_SECONDS: i64 = 60;
const PRUNE_INTERVAL_SECONDS: i64 = 24 * 3600;
const HEAVY_APP_BYTES: u64 = 500 * 1024 * 1024;

// The screensaver is a terminal window; time spent on it is nobody using the terminal.
const SCREENSAVER_CLASS: &str = "org.omarchy.screensaver";

fn main() -> Result<(), Box<dyn Error>> {
    let mut store = Store::open_for_recording()?;
    let mut hyprland = Hyprland::connect()?;
    let power_events = PowerEvents::listen()?;

    let mut now = unix_time();
    store.delete_older_than(now - RETENTION_SECONDS)?;
    let mut pruned_at = now;

    let mut power = power::read_power_state()?;
    let mut apps_with_known_class = HashSet::new();
    remember_window_classes(&hyprland, &store, &mut apps_with_known_class)?;
    let mut focus = FocusTimer::starting_at(now);
    focus.switch_to(focused_app(&hyprland, &store)?, now);

    let first_sample = Sample {
        ts: now,
        battery_percent: power.battery_percent,
        plugged: power.plugged,
        screen_on: hyprland.screen_is_on()?,
    };
    store.record(&first_sample, &[])?;
    let mut recorded_at = now;

    loop {
        let until_next_sample = (recorded_at + SAMPLE_INTERVAL_SECONDS - unix_time()).clamp(0, SAMPLE_INTERVAL_SECONDS);
        wait_for_input(&[hyprland.events_fd(), power_events.fd()], until_next_sample)?;
        now = unix_time();

        if hyprland.focus_changed()? {
            focus.switch_to(focused_app(&hyprland, &store)?, now);
        }

        let was_plugged = power.plugged;
        if power_events.power_supply_changed() {
            power = power::read_power_state()?;
        }

        let elapsed = now - recorded_at;
        if elapsed < SAMPLE_INTERVAL_SECONDS && power.plugged == was_plugged {
            continue;
        }

        power = power::read_power_state()?;
        let screen_on = hyprland.screen_is_on()?;
        let focused_seconds = focus.take_totals(now);

        // Nothing was in use while the screen was off or the machine was suspended.
        let was_in_use = screen_on && elapsed <= MAX_AWAKE_GAP_SECONDS;
        let apps = if was_in_use { apps_in_use(focused_seconds, elapsed) } else { Vec::new() };
        if was_in_use {
            remember_window_classes(&hyprland, &store, &mut apps_with_known_class)?;
        }

        let sample = Sample { ts: now, battery_percent: power.battery_percent, plugged: power.plugged, screen_on };
        store.record(&sample, &apps)?;
        recorded_at = now;

        if now - pruned_at >= PRUNE_INTERVAL_SECONDS {
            store.delete_older_than(now - RETENTION_SECONDS)?;
            pruned_at = now;
        }
    }
}

/// An app counts as in use for the time one of its windows had focus, or for the
/// whole stretch if it is holding a lot of memory.
fn apps_in_use(focused_seconds: HashMap<String, i64>, elapsed: i64) -> Vec<AppSeconds> {
    let mut apps: Vec<AppSeconds> = focused_seconds
        .into_iter()
        .map(|(app, seconds)| {
            let focused_seconds = seconds.min(elapsed);
            AppSeconds { app, seconds: focused_seconds, focused_seconds }
        })
        .collect();

    for heavy_app in processes::apps_using_at_least(HEAVY_APP_BYTES) {
        match apps.iter_mut().find(|usage| usage.app == heavy_app) {
            Some(usage) => usage.seconds = elapsed,
            None => apps.push(AppSeconds { app: heavy_app, seconds: elapsed, focused_seconds: 0 }),
        }
    }
    apps
}

fn focused_app(hyprland: &Hyprland, store: &Store) -> Result<Option<String>, Box<dyn Error>> {
    let Some(window) = hyprland.focused_window()? else {
        return Ok(None);
    };
    if window.class == SCREENSAVER_CLASS {
        return Ok(None);
    }
    let Some(app) = processes::app_name(window.pid) else {
        return Ok(None);
    };
    // The app looks up its display name and icon by window class.
    store.remember_window_class(&app, &window.class)?;
    Ok(Some(app))
}

/// Notes the window class of every open app, so that apps which never get focus
/// can still be shown with their proper name and icon.
fn remember_window_classes(hyprland: &Hyprland, store: &Store, apps_with_known_class: &mut HashSet<String>) -> Result<(), Box<dyn Error>> {
    for window in hyprland.open_windows()? {
        if window.class == SCREENSAVER_CLASS {
            continue;
        }
        if let Some(app) = processes::app_name(window.pid)
            && !apps_with_known_class.contains(&app)
        {
            store.remember_window_class(&app, &window.class)?;
            apps_with_known_class.insert(app);
        }
    }
    Ok(())
}

/// Adds up how long each app's window held focus between two samples.
struct FocusTimer {
    focused_app: Option<String>,
    focused_since: i64,
    seconds_by_app: HashMap<String, i64>,
}

impl FocusTimer {
    fn starting_at(now: i64) -> Self {
        Self { focused_app: None, focused_since: now, seconds_by_app: HashMap::new() }
    }

    fn switch_to(&mut self, app: Option<String>, now: i64) {
        self.credit_focused_app(now);
        self.focused_app = app;
    }

    fn take_totals(&mut self, now: i64) -> HashMap<String, i64> {
        self.credit_focused_app(now);
        std::mem::take(&mut self.seconds_by_app)
    }

    fn credit_focused_app(&mut self, now: i64) {
        if let Some(app) = &self.focused_app {
            *self.seconds_by_app.entry(app.clone()).or_default() += now - self.focused_since;
        }
        self.focused_since = now;
    }
}

fn wait_for_input(fds: &[RawFd], timeout_seconds: i64) -> io::Result<()> {
    let mut poll_fds: Vec<libc::pollfd> = fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    // SAFETY: `poll_fds` is a valid array of `poll_fds.len()` entries for the duration of the call.
    let ready = unsafe { libc::poll(poll_fds.as_mut_ptr(), poll_fds.len() as libc::nfds_t, (timeout_seconds * 1000) as i32) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

fn unix_time() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since_epoch| since_epoch.as_secs() as i64)
}

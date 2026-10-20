use crate::chart::Chart;
use crate::desktop_apps::DesktopApps;
use crate::format;
use crate::meter::Meter;
use crate::sparkline::Sparkline;
use crate::theme::Palette;
use crate::timeline::{Session, Timeline};
use adw::prelude::*;
use gtk::{gdk, glib, pango};
use joule_store::{AppTotal, MAX_AWAKE_GAP_SECONDS, Store};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const REFRESH_SECONDS: u32 = 60;
const DAYS_SHOWN: i32 = 7;
const TODAY: usize = DAYS_SHOWN as usize - 1;
const WINDOW_WIDTH: i32 = 880;
const MINIMUM_WINDOW_SIZE: (i32, i32) = (720, 720);
const WIDE_LAYOUT_MIN_WIDTH: f64 = 1400.0;
const USAGE_COLUMN_WIDTH: i32 = 720;

pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let palette = Palette::load();
    palette.apply();

    // Hyprland floats and centres windows that cannot be resized when they appear, so Joule
    // opens as a compact card instead of being tiled across the whole screen.
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Joule")
        .default_width(WINDOW_WIDTH)
        .default_height(window_height())
        .resizable(false)
        .build();
    // A window with breakpoints does not take its minimum size from its content.
    window.set_size_request(MINIMUM_WINDOW_SIZE.0, MINIMUM_WINDOW_SIZE.1);
    allow_resizing_once_shown(&window);

    let Ok(store) = Store::open_read_only() else {
        window.set_content(Some(&not_recording_notice()));
        return window;
    };

    let dashboard = Dashboard::new(store, palette);
    window.set_content(Some(&dashboard.root));
    window.add_breakpoint(dashboard.wide_layout.clone());
    window.add_controller(keyboard_shortcuts(&window, &dashboard));
    dashboard.reload(Change::Day);
    glib::timeout_add_seconds_local(REFRESH_SECONDS, move || {
        dashboard.reload(Change::NewData);
        glib::ControlFlow::Continue
    });
    window
}

/// Hyprland only looks at the fixed size when the window first appears. It is lifted right
/// after, because a window that cannot be resized ignores the size Hyprland gives it for
/// full screen and stays card-sized in a corner.
fn allow_resizing_once_shown(window: &adw::ApplicationWindow) {
    let frames_started = Cell::new(0);
    window.add_tick_callback(move |window, _| {
        frames_started.set(frames_started.get() + 1);
        // The first frame has only reached Hyprland once the second one starts.
        if frames_started.get() < 2 {
            return glib::ControlFlow::Continue;
        }
        window.set_resizable(true);
        glib::ControlFlow::Break
    });
}

/// Leaves room for the bar and gaps on the smallest connected monitor.
fn window_height() -> i32 {
    let smallest_monitor = gdk::Display::default()
        .and_then(|display| display.monitors().iter::<gdk::Monitor>().flatten().map(|monitor| monitor.geometry().height()).min())
        .unwrap_or(1080);
    (smallest_monitor - 140).clamp(MINIMUM_WINDOW_SIZE.1, 920)
}

fn not_recording_notice() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("battery-symbolic")
        .title("Nothing recorded yet")
        .description("Joule's collector has not run on this machine.\nStart it with: systemctl --user enable --now jouled")
        .build()
}

/// What prompted a redraw, which decides what animates.
#[derive(Clone, Copy, PartialEq)]
enum Change {
    Day,
    Session,
    NewData,
}

struct Day {
    start: i64,
    end: i64,
}

struct DayCell {
    button: gtk::ToggleButton,
    weekday: gtk::Label,
    date: gtk::Label,
    screen_time: gtk::Label,
    sparkline: Sparkline,
}

struct Selection {
    timeline: Rc<Timeline>,
    days: Vec<Day>,
    day: usize,
    session: Option<usize>,
    now: i64,
}

/// The figures shown above the app list for a selected day or session.
struct Summary {
    caption: String,
    screen_on_seconds: i64,
    stats: Vec<Stat>,
    from: i64,
    until: i64,
}

struct Stat {
    label: &'static str,
    value: String,
    swatch: Option<&'static str>,
}

struct Dashboard {
    root: gtk::Box,
    wide_layout: adw::Breakpoint,
    store: Store,
    desktop_apps: DesktopApps,
    palette: Palette,
    chart: Chart,
    power_status: gtk::Box,
    day_cells: Vec<DayCell>,
    session_scroller: gtk::ScrolledWindow,
    session_row: gtk::Box,
    session_chips: RefCell<Vec<(Option<usize>, gtk::Button)>>,
    caption: gtk::Label,
    shown_seconds: Rc<Cell<f64>>,
    total_animation: adw::TimedAnimation,
    stats: gtk::Box,
    app_list: gtk::ListBox,
    app_count: gtk::Label,
    app_meters: Rc<RefCell<Vec<Meter>>>,
    meter_growth: adw::TimedAnimation,
    selection: RefCell<Selection>,
}

impl Dashboard {
    fn new(store: Store, palette: Palette) -> Rc<Self> {
        let chart = Chart::new(palette.clone());
        let day_cells: Vec<DayCell> = (0..DAYS_SHOWN).map(|_| day_cell(&palette)).collect();
        for cell in &day_cells[1..] {
            cell.button.set_group(Some(&day_cells[0].button));
        }

        let power_status = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let week_strip = gtk::Box::builder().spacing(6).homogeneous(true).build();
        for cell in &day_cells {
            week_strip.append(&cell.button);
        }

        let battery_panel = notched_panel("BATTERY", &legend(), &chart.area);

        let session_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let session_scroller = gtk::ScrolledWindow::builder()
            .child(&session_row)
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .build();
        // The row has no scrollbar to drag, so let the mouse wheel scroll it sideways.
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        wheel.connect_scroll({
            let adjustment = session_scroller.hadjustment();
            move |_, _, delta_y| {
                adjustment.set_value(adjustment.value() + delta_y * adjustment.step_increment());
                glib::Propagation::Stop
            }
        });
        session_scroller.add_controller(wheel);

        let caption = text("", "caption");
        let total = text("", "total");
        let headline = gtk::Box::new(gtk::Orientation::Vertical, 0);
        headline.append(&caption);
        headline.append(&total);
        let stats = gtk::Box::builder().hexpand(true).halign(gtk::Align::End).valign(gtk::Align::End).build();
        stats.add_css_class("stats");
        let summary = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        summary.append(&headline);
        summary.append(&stats);

        let in_memory = legend_entry("in-memory", "in memory");
        in_memory.set_tooltip_text(Some("Not in focus, but counted because it held more than 500 MB of memory"));
        let app_count = gtk::Label::new(None);
        let apps_legend = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        apps_legend.append(&legend_entry("in-focus", "in focus"));
        apps_legend.append(&in_memory);
        apps_legend.append(&app_count);

        let app_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        app_list.add_css_class("app-list");
        app_list.set_placeholder(Some(&text("No apps were recorded in this period", "placeholder")));
        let app_scroller = gtk::ScrolledWindow::builder()
            .child(&app_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .min_content_height(100)
            .build();
        let apps_panel = notched_panel("APPS", &apps_legend, &app_scroller);

        let graph_column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        graph_column.append(&battery_panel);
        graph_column.append(&session_scroller);
        // Not expanding, so that in the wide layout all spare width goes to the graph.
        let usage_column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).hexpand(false).build();
        usage_column.append(&summary);
        usage_column.append(&apps_panel);
        let columns = gtk::Box::new(gtk::Orientation::Vertical, 12);
        columns.append(&graph_column);
        columns.append(&usage_column);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("dashboard");
        root.append(&header(&power_status));
        root.append(&week_strip);
        root.append(&columns);
        root.append(&key_hints());

        let wide_layout = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MinWidth,
            WIDE_LAYOUT_MIN_WIDTH,
            adw::LengthUnit::Px,
        ));
        wide_layout.add_setter(&columns, "orientation", Some(&gtk::Orientation::Horizontal.to_value()));
        wide_layout.add_setter(&chart.area, "vexpand", Some(&true.to_value()));
        wide_layout.add_setter(&usage_column, "width-request", Some(&USAGE_COLUMN_WIDTH.to_value()));

        let shown_seconds = Rc::new(Cell::new(0.0));
        let total_target = adw::CallbackAnimationTarget::new({
            let shown_seconds = shown_seconds.clone();
            move |seconds| {
                shown_seconds.set(seconds);
                total.set_label(&format::duration(seconds.round() as i64));
            }
        });
        let total_animation = adw::TimedAnimation::builder()
            .widget(&headline)
            .value_from(0.0)
            .value_to(0.0)
            .duration(350)
            .easing(adw::Easing::EaseOutCubic)
            .target(&total_target)
            .build();

        let app_meters: Rc<RefCell<Vec<Meter>>> = Rc::default();
        let meter_target = adw::CallbackAnimationTarget::new({
            let app_meters = app_meters.clone();
            move |progress| {
                for meter in app_meters.borrow().iter() {
                    meter.set_growth(progress);
                }
            }
        });
        let meter_growth = adw::TimedAnimation::builder()
            .widget(&app_list)
            .value_from(0.0)
            .value_to(1.0)
            .duration(500)
            .easing(adw::Easing::EaseOutCubic)
            .target(&meter_target)
            .build();

        let dashboard = Rc::new(Self {
            root,
            wide_layout,
            store,
            desktop_apps: DesktopApps::scan(),
            palette,
            chart,
            power_status,
            day_cells,
            session_scroller,
            session_row,
            session_chips: RefCell::default(),
            caption,
            shown_seconds,
            total_animation,
            stats,
            app_list,
            app_count,
            app_meters,
            meter_growth,
            selection: RefCell::new(Selection {
                timeline: Rc::default(),
                days: last_days(),
                day: TODAY,
                session: None,
                now: 0,
            }),
        });

        for (index, cell) in dashboard.day_cells.iter().enumerate() {
            let dashboard = dashboard.clone();
            cell.button.connect_clicked(move |_| dashboard.select_day(index));
        }
        dashboard.chart.connect_session_clicked({
            let dashboard = dashboard.clone();
            move |clicked| {
                // Clicking the selected session again, or empty space, returns to the whole day.
                let already_selected = dashboard.selection.borrow().session == clicked;
                dashboard.select_session(clicked.filter(|_| !already_selected));
            }
        });
        dashboard
    }

    fn reload(self: &Rc<Self>, change: Change) {
        let samples = match self.store.samples_since(0) {
            Ok(samples) => samples,
            Err(error) => {
                eprintln!("joule: could not read recorded samples: {error}");
                return;
            }
        };
        let timeline = Rc::new(Timeline::build(&samples));

        {
            let mut selection = self.selection.borrow_mut();
            // Session numbers shift as old data is pruned, so follow the session by its start time.
            let selected_start = selection.session.map(|session| selection.timeline.sessions[session].start);
            selection.session = selected_start.and_then(|start| timeline.sessions.iter().position(|session| session.start == start));
            selection.timeline = timeline;
            selection.days = last_days();
            selection.now = glib::real_time() / 1_000_000;
        }

        let selection = self.selection.borrow();
        for (day, cell) in selection.days.iter().zip(&self.day_cells) {
            let screen_on = selection.timeline.screen_on_seconds(day.start, day.end, None);
            let is_today = (day.start..day.end).contains(&selection.now);
            cell.weekday.set_label(&if is_today { "TODAY".to_owned() } else { format::local_time(day.start, "%a").to_uppercase() });
            cell.date.set_label(&format::local_time(day.start, "%-d"));
            cell.screen_time.set_label(&if screen_on > 0 { format::duration(screen_on) } else { "–".to_owned() });
            cell.sparkline.show_day(selection.timeline.clone(), day.start, day.end, selection.now);
        }
        self.day_cells[selection.day].button.set_active(true);
        self.show_power_status(&selection.timeline, selection.now);
        drop(selection);

        self.show(change);
    }

    fn select_day(self: &Rc<Self>, day: usize) {
        {
            let mut selection = self.selection.borrow_mut();
            selection.day = day;
            selection.session = None;
        }
        self.day_cells[day].button.set_active(true);
        self.show(Change::Day);
    }

    fn step_day(self: &Rc<Self>, step: i32) {
        let day = self.selection.borrow().day as i32 + step;
        if (0..DAYS_SHOWN).contains(&day) {
            self.select_day(day as usize);
        }
    }

    fn select_session(self: &Rc<Self>, session: Option<usize>) {
        self.selection.borrow_mut().session = session;
        self.show(Change::Session);
    }

    /// Moves through the day's sessions in order, passing through "whole day" at either end.
    fn step_session(self: &Rc<Self>, step: i32) {
        let next = {
            let selection = self.selection.borrow();
            let day = &selection.days[selection.day];
            let sessions = selection.timeline.sessions_between(day.start, day.end.min(selection.now));
            let position = selection.session.and_then(|selected| sessions.iter().position(|&session| session == selected));
            let next_position = match position {
                Some(position) => position as i32 + step,
                None if step > 0 => 0,
                None => sessions.len() as i32 - 1,
            };
            usize::try_from(next_position).ok().and_then(|position| sessions.get(position).copied())
        };
        self.select_session(next);
    }

    fn show(self: &Rc<Self>, change: Change) {
        let selection = self.selection.borrow();
        let day = &selection.days[selection.day];
        self.chart.show_day(selection.timeline.clone(), day.start, day.end, selection.now, change == Change::Day);
        self.chart.select_session(selection.session);

        let summary = match selection.session {
            Some(session) => session_summary(&selection.timeline.sessions[session], day, selection.now),
            None => day_summary(&selection.timeline, day, selection.now),
        };
        self.caption.set_label(&summary.caption);
        self.show_total(summary.screen_on_seconds, change != Change::NewData);
        clear(&self.stats);
        for (index, stat) in summary.stats.iter().enumerate() {
            if index > 0 {
                self.stats.append(&gtk::Separator::new(gtk::Orientation::Vertical));
            }
            self.stats.append(&stat_cell(stat));
        }

        if change == Change::Session {
            self.mark_selected_chip(selection.session);
        } else {
            self.rebuild_session_chips(&selection);
        }
        drop(selection);

        let apps = self.store.app_totals(summary.from, summary.until).unwrap_or_else(|error| {
            eprintln!("joule: could not read app usage: {error}");
            Vec::new()
        });
        self.show_apps(&apps, change != Change::NewData);
    }

    fn show_power_status(&self, timeline: &Timeline, now: i64) {
        clear(&self.power_status);
        let Some(session) = timeline.sessions.last().filter(|session| is_live(session, now)) else {
            self.power_status.set_tooltip_text(Some("The collector is not running. Start it with: systemctl --user enable --now jouled"));
            self.power_status.append(&text("NOT RECORDING", "dim"));
            return;
        };

        self.power_status.set_tooltip_text(None);
        let power = if session.plugged { "PLUGGED IN" } else { "ON BATTERY" };
        self.power_status.append(&swatch(if session.plugged { "plugged" } else { "on-battery" }));
        self.power_status.append(&text(&format!("{power} {}%", session.end_percent), "power-state"));
        self.power_status.append(&text(&format!("for {}", format::duration(now - session.start)), "dim"));
    }

    /// Counts the figure up or down from whatever is currently displayed.
    fn show_total(&self, seconds: i64, animate: bool) {
        self.total_animation.set_value_from(self.shown_seconds.get());
        self.total_animation.set_value_to(seconds as f64);
        self.total_animation.play();
        if !animate {
            self.total_animation.skip();
        }
    }

    fn rebuild_session_chips(self: &Rc<Self>, selection: &Selection) {
        clear(&self.session_row);
        let day = &selection.days[selection.day];
        let mut chips = vec![(None, chip(&gtk::Label::new(Some("WHOLE DAY"))))];
        for index in selection.timeline.sessions_between(day.start, day.end.min(selection.now)) {
            chips.push((Some(index), session_chip(&selection.timeline.sessions[index], day, selection.now)));
        }

        for &(session, ref chip) in &chips {
            let dashboard = self.clone();
            chip.connect_clicked(move |_| dashboard.select_session(session));
            self.session_row.append(chip);
        }
        self.session_chips.replace(chips);
        self.mark_selected_chip(selection.session);
    }

    fn mark_selected_chip(&self, selected: Option<usize>) {
        for (session, chip) in self.session_chips.borrow().iter() {
            if *session == selected {
                chip.add_css_class("selected");
                self.scroll_into_view(chip);
            } else {
                chip.remove_css_class("selected");
            }
        }
    }

    fn scroll_into_view(&self, chip: &gtk::Button) {
        let (chip, row, adjustment) = (chip.clone(), self.session_row.clone(), self.session_scroller.hadjustment());
        // Deferred because a freshly rebuilt row has not been laid out yet.
        glib::idle_add_local_once(move || {
            let Some(bounds) = chip.compute_bounds(&row) else {
                return;
            };
            let (left, right) = (bounds.x() as f64, (bounds.x() + bounds.width()) as f64);
            adjustment.set_value(adjustment.value().max(right - adjustment.page_size()).min(left));
        });
    }

    fn show_apps(&self, apps: &[AppTotal], animate: bool) {
        // Anything under half a minute would be listed as "0m".
        let apps: Vec<&AppTotal> = apps.iter().filter(|app| app.seconds >= 30).collect();
        let longest = apps.iter().map(|app| app.seconds).max().unwrap_or(1) as f64;
        self.app_count.set_label(&format!("{} {}", apps.len(), if apps.len() == 1 { "app" } else { "apps" }));

        self.app_list.remove_all();
        let name_cells = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        let mut meters = Vec::new();
        for (index, app) in apps.into_iter().enumerate() {
            let meter = Meter::new(&self.palette);
            meter.set_shares(app.focused_seconds as f64 / longest, app.seconds as f64 / longest);
            if animate {
                meter.set_growth(0.0);
            }
            self.app_list.append(&self.app_row(index + 1, app, &meter, &name_cells));
            meters.push(meter);
        }
        self.app_meters.replace(meters);
        if animate {
            self.meter_growth.play();
        }
    }

    fn app_row(&self, rank: usize, app: &AppTotal, meter: &Meter, name_cells: &gtk::SizeGroup) -> gtk::ListBoxRow {
        let identity = self.desktop_apps.identify(&app.app, app.window_class.as_deref());

        let icon = gtk::Image::builder().pixel_size(22).build();
        if identity.icon.starts_with('/') {
            icon.set_from_file(Some(&identity.icon));
        } else {
            icon.set_icon_name(Some(&identity.icon));
        }

        let name = text(&identity.name, "app-name");
        name.set_max_width_chars(30);
        name.set_ellipsize(pango::EllipsizeMode::End);
        let name_cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        name_cell.append(&name);
        if app.focused_seconds == 0 {
            let tag = text("background", "tag");
            tag.set_valign(gtk::Align::Center);
            tag.set_tooltip_text(Some("Never in focus; counted because it held more than 500 MB of memory"));
            name_cell.append(&tag);
        }
        // Equal widths keep the meters of all rows starting in the same place.
        name_cells.add_widget(&name_cell);

        meter.area.set_valign(gtk::Align::Center);
        let time = text(&format::duration(app.seconds), "app-time");
        time.set_width_chars(7);
        time.set_xalign(1.0);

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        content.add_css_class("app-row");
        content.append(&text(&format!("{rank:02}"), "dim"));
        content.append(&icon);
        content.append(&name_cell);
        content.append(&meter.area);
        content.append(&time);
        gtk::ListBoxRow::builder().child(&content).activatable(false).focusable(false).build()
    }
}

fn keyboard_shortcuts(window: &adw::ApplicationWindow, dashboard: &Rc<Dashboard>) -> gtk::EventControllerKey {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let dashboard = dashboard.clone();
        // Weak, because the window owns this controller.
        let window = window.downgrade();
        move |_, key, _, modifiers| {
            let close_window = || {
                if let Some(window) = window.upgrade() {
                    window.close();
                }
            };
            match key {
                gdk::Key::Left | gdk::Key::h => dashboard.step_day(-1),
                gdk::Key::Right | gdk::Key::l => dashboard.step_day(1),
                gdk::Key::Down | gdk::Key::j => dashboard.step_session(1),
                gdk::Key::Up | gdk::Key::k => dashboard.step_session(-1),
                gdk::Key::t | gdk::Key::Home => dashboard.select_day(TODAY),
                gdk::Key::Escape if dashboard.selection.borrow().session.is_some() => dashboard.select_session(None),
                gdk::Key::Escape | gdk::Key::q => close_window(),
                gdk::Key::w if modifiers.contains(gdk::ModifierType::CONTROL_MASK) => close_window(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        }
    });
    keys
}

/// Whether the collector has recorded the session recently enough for it to be still going.
fn is_live(session: &Session, now: i64) -> bool {
    now - session.end <= MAX_AWAKE_GAP_SECONDS
}

/// A time of day, with the weekday added when it falls outside the given day.
fn clock_time(ts: i64, day: &Day) -> String {
    let pattern = if (day.start..=day.end).contains(&ts) { "%H:%M" } else { "%a %H:%M" };
    format::local_time(ts, pattern).to_uppercase()
}

fn day_summary(timeline: &Timeline, day: &Day, now: i64) -> Summary {
    let is_today = (day.start..day.end).contains(&now);
    let date = if is_today { "TODAY".to_owned() } else { format::local_time(day.start, "%A %-d %B").to_uppercase() };
    let screen_on = |plugged| timeline.screen_on_seconds(day.start, day.end, plugged);

    Summary {
        caption: format!("SCREEN TIME  ·  {date}"),
        screen_on_seconds: screen_on(None),
        stats: vec![
            Stat { label: "ON BATTERY", value: format::duration(screen_on(Some(false))), swatch: Some("on-battery") },
            Stat { label: "PLUGGED IN", value: format::duration(screen_on(Some(true))), swatch: Some("plugged") },
            Stat { label: "BATTERY USED", value: format!("{:.0}%", timeline.battery_drained(day.start, day.end)), swatch: None },
        ],
        from: day.start,
        until: day.end,
    }
}

fn session_summary(session: &Session, day: &Day, now: i64) -> Summary {
    let end = if is_live(session, now) { "NOW".to_owned() } else { clock_time(session.end, day) };
    let power = if session.plugged { "PLUGGED IN" } else { "ON BATTERY" };

    let mut stats = vec![
        Stat { label: "DURATION", value: format::duration(session.end - session.start), swatch: None },
        Stat { label: "BATTERY", value: format!("{}% → {}%", session.start_percent, session.end_percent), swatch: None },
    ];
    let drained = session.start_percent.saturating_sub(session.end_percent);
    // Too little screen time makes the hourly figure swing wildly.
    if !session.plugged && session.screen_on_seconds >= 10 * 60 {
        let per_hour = drained as f64 * 3600.0 / session.screen_on_seconds as f64;
        stats.push(Stat { label: "USED PER SCREEN HOUR", value: format!("{per_hour:.0}%"), swatch: None });
    }

    Summary {
        caption: format!("{power}  ·  {} – {end}", clock_time(session.start, day)),
        screen_on_seconds: session.screen_on_seconds,
        stats,
        from: session.start,
        until: session.end,
    }
}

fn last_days() -> Vec<Day> {
    let now = glib::DateTime::now_local().expect("the local time zone is available");
    let today = glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.0).expect("today is a valid date");
    (0..DAYS_SHOWN)
        .rev()
        .map(|days_ago| {
            let midnight = |days: i32| today.add_days(days).expect("dates this week are valid").to_unix();
            Day { start: midnight(-days_ago), end: midnight(1 - days_ago) }
        })
        .collect()
}

fn header(power_status: &gtk::Box) -> gtk::Box {
    let titles = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    titles.append(&text("JOULE", "wordmark"));
    titles.append(&text("battery · screen time", "dim"));
    power_status.set_hexpand(true);
    power_status.set_halign(gtk::Align::End);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    header.append(&titles);
    header.append(power_status);
    header
}

fn day_cell(palette: &Palette) -> DayCell {
    let weekday = text("", "weekday");
    weekday.set_hexpand(true);
    let date = text("", "date");
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    heading.append(&weekday);
    heading.append(&date);

    let sparkline = Sparkline::new(palette);
    let screen_time = text("", "screen-time");
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.append(&heading);
    content.append(&sparkline.area);
    content.append(&screen_time);

    let button = gtk::ToggleButton::builder().child(&content).focusable(false).build();
    button.add_css_class("day");
    DayCell { button, weekday, date, screen_time, sparkline }
}

/// A bordered panel whose title, and a widget on the right, interrupt the top border
/// like the box titles in btop.
fn notched_panel(title: &str, trailing: &impl IsA<gtk::Widget>, content: &impl IsA<gtk::Widget>) -> gtk::Overlay {
    let panel = gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_top(8).build();
    panel.add_css_class("panel");
    panel.append(content);

    let title = gtk::Label::builder().label(title).halign(gtk::Align::Start).valign(gtk::Align::Start).margin_start(14).build();
    title.add_css_class("panel-title");
    trailing.add_css_class("panel-notch");
    trailing.set_halign(gtk::Align::End);
    trailing.set_valign(gtk::Align::Start);
    trailing.set_margin_end(14);

    let overlay = gtk::Overlay::builder().child(&panel).build();
    overlay.add_overlay(&title);
    overlay.add_overlay(trailing);
    overlay
}

fn chip(content: &impl IsA<gtk::Widget>) -> gtk::Button {
    let chip = gtk::Button::builder().child(content).focusable(false).build();
    chip.add_css_class("chip");
    chip
}

fn session_chip(session: &Session, day: &Day, now: i64) -> gtk::Button {
    let end = if is_live(session, now) { now } else { session.end };
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.append(&swatch(if session.plugged { "plugged" } else { "on-battery" }));
    content.append(&gtk::Label::new(Some(&format!("{}–{}", clock_time(session.start, day), clock_time(end, day)))));
    content.append(&text(&format!("{}→{}%", session.start_percent, session.end_percent), "dim"));
    chip(&content)
}

fn legend() -> gtk::Box {
    let legend = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    for (swatch_class, meaning) in [("plugged", "Plugged in"), ("on-battery", "On battery"), ("screen-off", "Screen off")] {
        legend.append(&legend_entry(swatch_class, meaning));
    }
    legend
}

fn legend_entry(swatch_class: &str, meaning: &str) -> gtk::Box {
    let entry = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    entry.append(&swatch(swatch_class));
    entry.append(&gtk::Label::new(Some(meaning)));
    entry
}

fn key_hints() -> gtk::Box {
    let hints = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    hints.add_css_class("hints");
    for (keys, action) in [("← →", "day"), ("↑ ↓", "session"), ("t", "today"), ("esc", "back"), ("q", "close")] {
        let hint = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hint.append(&text(keys, "key"));
        hint.append(&gtk::Label::new(Some(action)));
        hints.append(&hint);
    }
    hints
}

fn stat_cell(stat: &Stat) -> gtk::Box {
    let label_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    if let Some(swatch_class) = stat.swatch {
        label_row.append(&swatch(swatch_class));
    }
    label_row.append(&text(stat.label, "stat-label"));

    let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
    cell.add_css_class("stat");
    cell.append(&label_row);
    cell.append(&text(&stat.value, "stat-value"));
    cell
}

fn swatch(css_class: &str) -> gtk::Box {
    let swatch = gtk::Box::builder().valign(gtk::Align::Center).build();
    swatch.add_css_class("swatch");
    swatch.add_css_class(css_class);
    swatch
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn text(content: &str, css_class: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(content).xalign(0.0).build();
    label.add_css_class(css_class);
    label
}

use crate::format;
use crate::theme::{Color, ON_BATTERY, PLUGGED, Palette};
use crate::timeline::{Activity, Stretch, Timeline};
use adw::prelude::*;
use gtk::{cairo, pango};
use std::cell::RefCell;
use std::rc::Rc;

const Y_LABEL_GAP: f64 = 10.0;
const MARGIN_RIGHT: f64 = 20.0;
const MARGIN_TOP: f64 = 16.0;
const MARGIN_BOTTOM: f64 = 30.0;

const HOUR: i64 = 3600;

/// 1px dots with a 4px gap, for grid lines.
const DOTTED: [f64; 2] = [1.0, 4.0];

struct View {
    palette: Palette,
    timeline: Rc<Timeline>,
    day_start: i64,
    day_end: i64,
    now: i64,
    /// Kept after deselecting so the highlight can fade out; `focus` says how visible it is.
    selected_session: Option<usize>,
    selection_active: bool,
    pointer: Option<(f64, f64)>,
    /// 0 to 1 while a newly shown day wipes in from the left.
    reveal: f64,
    /// 0 to 1 as everything outside the selected session fades back.
    focus: f64,
}

/// The battery graph for one day: the charge curve over bands showing whether the
/// laptop was plugged in, on battery, or had its screen off.
#[derive(Clone)]
pub struct Chart {
    pub area: gtk::DrawingArea,
    view: Rc<RefCell<View>>,
    reveal: adw::TimedAnimation,
    focus: adw::TimedAnimation,
    session_clicked: Rc<RefCell<Option<SessionClicked>>>,
}

type SessionClicked = Box<dyn Fn(Option<usize>)>;

impl Chart {
    pub fn new(palette: Palette) -> Self {
        let area = gtk::DrawingArea::builder().hexpand(true).height_request(230).build();
        let view = Rc::new(RefCell::new(View {
            palette,
            timeline: Rc::default(),
            day_start: 0,
            day_end: 1,
            now: 0,
            selected_session: None,
            selection_active: false,
            pointer: None,
            reveal: 1.0,
            focus: 0.0,
        }));

        area.set_draw_func({
            let view = view.clone();
            move |area, cr, width, height| {
                let plot = Plot::new(area, width as f64, height as f64, &view.borrow());
                // A cairo error leaves the context unusable; there is nothing to recover mid-frame.
                let _ = draw(area, cr, &plot, &view.borrow());
            }
        });

        let reveal = animation(&area, &view, 600, |view, progress| view.reveal = progress);
        let focus = animation(&area, &view, 220, |view, progress| view.focus = progress);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let (area, view) = (area.clone(), view.clone());
            move |_, x, y| {
                view.borrow_mut().pointer = Some((x, y));
                let over_session = session_under_pointer(&area, &view.borrow()).is_some();
                area.set_cursor_from_name(over_session.then_some("pointer"));
                area.queue_draw();
            }
        });
        motion.connect_leave({
            let (area, view) = (area.clone(), view.clone());
            move |_| {
                view.borrow_mut().pointer = None;
                area.queue_draw();
            }
        });
        area.add_controller(motion);

        let session_clicked: Rc<RefCell<Option<SessionClicked>>> = Rc::default();
        let click = gtk::GestureClick::new();
        click.connect_released({
            let (area, view, session_clicked) = (area.clone(), view.clone(), session_clicked.clone());
            move |_, _, x, y| {
                view.borrow_mut().pointer = Some((x, y));
                let session = session_under_pointer(&area, &view.borrow());
                if let Some(session_clicked) = session_clicked.borrow().as_ref() {
                    session_clicked(session);
                }
            }
        });
        area.add_controller(click);

        Self { area, view, reveal, focus, session_clicked }
    }

    /// Calls `handler` with the session under each click, or `None` for a click outside any session.
    pub fn connect_session_clicked(&self, handler: impl Fn(Option<usize>) + 'static) {
        self.session_clicked.replace(Some(Box::new(handler)));
    }

    pub fn show_day(&self, timeline: Rc<Timeline>, day_start: i64, day_end: i64, now: i64, animate: bool) {
        {
            let mut view = self.view.borrow_mut();
            view.timeline = timeline;
            view.day_start = day_start;
            view.day_end = day_end;
            view.now = now;
        }
        if animate {
            self.reveal.play();
        }
        self.area.queue_draw();
    }

    pub fn select_session(&self, session: Option<usize>) {
        let was_active = {
            let mut view = self.view.borrow_mut();
            if session.is_some() {
                view.selected_session = session;
            }
            std::mem::replace(&mut view.selection_active, session.is_some())
        };
        if session.is_some() != was_active {
            self.focus.set_reverse(session.is_none());
            self.focus.play();
        }
        self.area.queue_draw();
    }
}

fn animation(
    area: &gtk::DrawingArea,
    view: &Rc<RefCell<View>>,
    milliseconds: u32,
    apply: impl Fn(&mut View, f64) + 'static,
) -> adw::TimedAnimation {
    let target = adw::CallbackAnimationTarget::new({
        let (area, view) = (area.clone(), view.clone());
        move |progress| {
            apply(&mut view.borrow_mut(), progress);
            area.queue_draw();
        }
    });
    adw::TimedAnimation::builder()
        .widget(area)
        .value_from(0.0)
        .value_to(1.0)
        .duration(milliseconds)
        .easing(adw::Easing::EaseOutCubic)
        .target(&target)
        .build()
}

fn session_under_pointer(area: &gtk::DrawingArea, view: &View) -> Option<usize> {
    let plot = Plot::new(area, area.width() as f64, area.height() as f64, view);
    let (x, y) = view.pointer?;
    if !plot.contains(x, y) {
        return None;
    }
    view.timeline.stretch_at(plot.ts(x))?.session
}

/// Where the day is drawn inside the widget, and how time and charge map onto it.
struct Plot {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    day_start: i64,
    day_end: i64,
}

impl Plot {
    fn new(area: &gtk::DrawingArea, widget_width: f64, widget_height: f64, view: &View) -> Self {
        let left = label(area, "100%").pixel_size().0 as f64 + Y_LABEL_GAP;
        Self {
            left,
            top: MARGIN_TOP,
            width: (widget_width - left - MARGIN_RIGHT).max(1.0),
            height: (widget_height - MARGIN_TOP - MARGIN_BOTTOM).max(1.0),
            day_start: view.day_start,
            day_end: view.day_end,
        }
    }

    fn right(&self) -> f64 {
        self.left + self.width
    }

    fn bottom(&self) -> f64 {
        self.top + self.height
    }

    fn x(&self, ts: i64) -> f64 {
        self.left + self.width * (ts - self.day_start) as f64 / (self.day_end - self.day_start) as f64
    }

    fn y(&self, percent: f64) -> f64 {
        self.bottom() - self.height * percent / 100.0
    }

    fn ts(&self, x: f64) -> i64 {
        self.day_start + ((x - self.left) / self.width * (self.day_end - self.day_start) as f64) as i64
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        (self.left..=self.right()).contains(&x) && (self.top..=self.bottom()).contains(&y)
    }
}

fn draw(area: &gtk::DrawingArea, cr: &cairo::Context, plot: &Plot, view: &View) -> Result<(), cairo::Error> {
    draw_grid(area, cr, plot, view)?;

    let visible_until = view.now.min(view.day_end);
    let stretches: Vec<Stretch> = view.timeline.stretches_between(view.day_start, visible_until).collect();
    if stretches.is_empty() {
        return draw_empty_notice(area, cr, plot, view);
    }

    cr.save()?;
    cr.rectangle(plot.left, 0.0, plot.width * view.reveal, plot.bottom() + 1.0);
    cr.clip();
    for stretch in &stretches {
        draw_stretch(cr, plot, view, stretch)?;
    }
    draw_session_frames(cr, plot, view, visible_until)?;
    draw_now_marker(cr, plot, view)?;
    cr.restore()?;

    draw_pointer_readout(area, cr, plot, view)
}

fn draw_grid(area: &gtk::DrawingArea, cr: &cairo::Context, plot: &Plot, view: &View) -> Result<(), cairo::Error> {
    let ink = view.palette.foreground;
    cr.set_line_width(1.0);

    for percent in [0.0, 25.0, 50.0, 75.0, 100.0] {
        let y = plot.y(percent).round() + 0.5;
        if percent == 0.0 {
            set_color(cr, ink, 0.22);
            cr.set_dash(&[], 0.0);
        } else {
            set_color(cr, ink, 0.16);
            cr.set_dash(&DOTTED, 0.0);
        }
        cr.move_to(plot.left, y);
        cr.line_to(plot.right(), y);
        cr.stroke()?;

        let label = label(area, &format!("{percent}%"));
        let (label_width, label_height) = label.pixel_size();
        set_color(cr, ink, 0.5);
        cr.move_to(plot.left - Y_LABEL_GAP - label_width as f64, y - label_height as f64 / 2.0);
        pangocairo::functions::show_layout(cr, &label);
    }

    cr.set_dash(&DOTTED, 0.0);
    let hours_per_label = if plot.width >= 620.0 { 3 } else { 6 };
    for hour in (0..=24).step_by(hours_per_label) {
        let ts = view.day_start + hour * HOUR;
        let x = plot.x(ts).round() + 0.5;
        set_color(cr, ink, 0.09);
        cr.move_to(x, plot.top);
        cr.line_to(x, plot.bottom());
        cr.stroke()?;

        let text = if hour == 24 { "24:00".to_owned() } else { format::local_time(ts, "%H:%M") };
        let label = label(area, &text);
        let label_width = label.pixel_size().0 as f64;
        let label_left = (x - label_width / 2.0).min(plot.right() + MARGIN_RIGHT - label_width - 2.0);
        set_color(cr, ink, 0.5);
        cr.move_to(label_left, plot.bottom() + 9.0);
        pangocairo::functions::show_layout(cr, &label);
    }
    cr.set_dash(&[], 0.0);
    Ok(())
}

fn draw_empty_notice(area: &gtk::DrawingArea, cr: &cairo::Context, plot: &Plot, view: &View) -> Result<(), cairo::Error> {
    let notice = area.create_pango_layout(Some("Nothing was recorded on this day"));
    let (notice_width, notice_height) = notice.pixel_size();
    set_color(cr, view.palette.foreground, 0.4);
    cr.move_to(plot.left + (plot.width - notice_width as f64) / 2.0, plot.top + (plot.height - notice_height as f64) / 2.0);
    pangocairo::functions::show_layout(cr, &notice);
    Ok(())
}

fn draw_stretch(cr: &cairo::Context, plot: &Plot, view: &View, stretch: &Stretch) -> Result<(), cairo::Error> {
    let (left, right) = (plot.x(stretch.start), plot.x(stretch.end));
    let session_color = stretch.session.map(|session| power_color(view.timeline.sessions[session].plugged));
    let outside_selection = view.selected_session.is_some() && stretch.session != view.selected_session;
    let strength = if outside_selection { 1.0 - 0.72 * view.focus } else { 1.0 };

    let curve: Vec<(f64, f64)> = view
        .timeline
        .battery_curve_between(stretch.start, stretch.end)
        .into_iter()
        .map(|(ts, percent)| (plot.x(ts), plot.y(percent)))
        .collect();

    match (stretch.activity, session_color) {
        (Activity::ScreenOn, Some(color)) => {
            if let (Some(first), Some(last)) = (curve.first(), curve.last()) {
                trace(cr, &curve);
                cr.line_to(last.0, plot.bottom());
                cr.line_to(first.0, plot.bottom());
                cr.close_path();
                let fill = cairo::LinearGradient::new(0.0, plot.top, 0.0, plot.bottom());
                fill.add_color_stop_rgba(0.0, color.red, color.green, color.blue, 0.46 * strength);
                fill.add_color_stop_rgba(1.0, color.red, color.green, color.blue, 0.07 * strength);
                cr.set_source(&fill)?;
                cr.fill()?;
            }
            trace(cr, &curve);
            set_color(cr, color, strength);
            cr.set_line_width(2.0);
            cr.set_line_join(cairo::LineJoin::Round);
            cr.stroke()?;
        }
        _ => {
            draw_screen_off_band(cr, plot, view, left, right)?;
            trace(cr, &curve);
            set_color(cr, view.palette.foreground, 0.32 * strength);
            cr.set_line_width(1.5);
            cr.set_dash(&[4.0, 4.0], 0.0);
            cr.stroke()?;
            cr.set_dash(&[], 0.0);
        }
    }

    // A strip along the baseline keeps a session readable across its screen-off parts.
    if let Some(color) = session_color {
        set_color(cr, color, 0.9 * strength);
        cr.rectangle(left, plot.bottom() - 3.0, right - left, 3.0);
        cr.fill()?;
    }
    Ok(())
}

fn draw_screen_off_band(cr: &cairo::Context, plot: &Plot, view: &View, left: f64, right: f64) -> Result<(), cairo::Error> {
    const HATCH_SPACING: f64 = 7.0;

    cr.save()?;
    cr.rectangle(left, plot.top, right - left, plot.height);
    cr.clip();
    set_color(cr, Color::BLACK, if view.palette.dark { 0.5 } else { 0.1 });
    cr.paint()?;

    set_color(cr, view.palette.foreground, 0.07);
    cr.set_line_width(1.0);
    // Anchored to the plot rather than the band so neighbouring bands line up.
    let mut x = plot.left - plot.height + ((left - plot.left) / HATCH_SPACING).floor() * HATCH_SPACING;
    while x < right {
        cr.move_to(x, plot.bottom());
        cr.line_to(x + plot.height, plot.top);
        x += HATCH_SPACING;
    }
    cr.stroke()?;
    cr.restore()
}

/// Outlines the selected session, and faintly the one under the pointer.
fn draw_session_frames(cr: &cairo::Context, plot: &Plot, view: &View, visible_until: i64) -> Result<(), cairo::Error> {
    let frame = |session: usize, fill_opacity: f64, outline_opacity: f64| -> Result<(), cairo::Error> {
        let session = &view.timeline.sessions[session];
        let left = plot.x(session.start.max(view.day_start)).round();
        let right = plot.x(session.end.min(visible_until)).round();
        if right <= left {
            return Ok(());
        }
        let color = power_color(session.plugged);
        rounded_rectangle(cr, left + 0.5, plot.top - 5.0 + 0.5, right - left - 1.0, plot.height + 5.0 - 1.0, view.palette.radius);
        set_color(cr, color, fill_opacity);
        cr.fill_preserve()?;
        set_color(cr, color, outline_opacity);
        cr.set_line_width(1.0);
        cr.stroke()
    };

    let hovered = view.pointer.and_then(|(x, y)| view.timeline.stretch_at(plot.ts(x)).filter(|_| plot.contains(x, y))?.session);
    if let Some(hovered) = hovered.filter(|&session| Some(session) != view.selected_session || view.focus < 1.0) {
        frame(hovered, 0.06, 0.0)?;
    }
    if let Some(selected) = view.selected_session {
        frame(selected, 0.08 * view.focus, 0.85 * view.focus)?;
    }
    Ok(())
}

fn draw_now_marker(cr: &cairo::Context, plot: &Plot, view: &View) -> Result<(), cairo::Error> {
    if view.now >= view.day_end {
        return Ok(());
    }

    set_color(cr, view.palette.foreground, 0.25);
    cr.set_line_width(1.0);
    cr.set_dash(&DOTTED, 0.0);
    let line_x = plot.x(view.now).round() + 0.5;
    cr.move_to(line_x, plot.top);
    cr.line_to(line_x, plot.bottom());
    cr.stroke()?;
    cr.set_dash(&[], 0.0);

    let Some(&(ts, percent)) = view.timeline.battery_curve_between(view.day_start, view.now).last() else {
        return Ok(());
    };
    let Some(session) = view.timeline.stretch_at(ts - 1).and_then(|stretch| stretch.session) else {
        return Ok(());
    };
    let color = power_color(view.timeline.sessions[session].plugged);
    let (x, y) = (plot.x(ts), plot.y(percent));
    set_color(cr, color, 0.25);
    cr.arc(x, y, 8.0, 0.0, std::f64::consts::TAU);
    cr.fill()?;
    set_color(cr, color, 1.0);
    cr.arc(x, y, 3.5, 0.0, std::f64::consts::TAU);
    cr.fill()
}

/// The crosshair and tooltip showing the time and charge under the pointer.
fn draw_pointer_readout(area: &gtk::DrawingArea, cr: &cairo::Context, plot: &Plot, view: &View) -> Result<(), cairo::Error> {
    let Some((x, y)) = view.pointer.filter(|&(x, y)| plot.contains(x, y)) else {
        return Ok(());
    };
    let ts = plot.ts(x);
    let ink = view.palette.foreground;

    set_color(cr, ink, 0.3);
    cr.set_line_width(1.0);
    cr.move_to(x.round() + 0.5, plot.top);
    cr.line_to(x.round() + 0.5, plot.bottom());
    cr.stroke()?;

    let stretch = view.timeline.stretch_at(ts).filter(|_| ts <= view.now);
    let percent = stretch.and_then(|_| view.timeline.battery_percent_at(ts));
    let power = stretch.and_then(|stretch| stretch.session).map(|session| view.timeline.sessions[session].plugged);

    if let Some(percent) = percent {
        set_color(cr, view.palette.background, 1.0);
        cr.arc(x, plot.y(percent), 6.5, 0.0, std::f64::consts::TAU);
        cr.fill()?;
        set_color(cr, power.map_or(ink, power_color), 1.0);
        cr.arc(x, plot.y(percent), 4.0, 0.0, std::f64::consts::TAU);
        cr.fill()?;
    }

    let state = match (stretch.map(|stretch| stretch.activity), power) {
        (Some(Activity::ScreenOn), Some(true)) => "Plugged in",
        (Some(Activity::ScreenOn), _) => "On battery",
        (Some(Activity::ScreenOff), Some(true)) => "Screen off · plugged in",
        (Some(Activity::ScreenOff), _) => "Screen off · on battery",
        (Some(Activity::Asleep), _) => "Asleep",
        (None, _) => "Nothing recorded",
    };
    let time = format::local_time(ts, "%H:%M");
    let headline = match percent {
        Some(percent) => format!("<b>{time}</b>   <b>{percent:.0}%</b>"),
        None => format!("<b>{time}</b>"),
    };
    let headline_layout = area.create_pango_layout(None);
    headline_layout.set_markup(&headline);
    // The dimming comes from the cairo colour, not a Pango alpha attribute: pangocairo keeps
    // that alpha on its shared renderer and GTK would then draw its own text dimmed too.
    let state_layout = area.create_pango_layout(None);
    state_layout.set_markup(&format!("<span size='small'>{state}</span>"));

    const PADDING_X: f64 = 10.0;
    const PADDING_Y: f64 = 8.0;
    const POINTER_GAP: f64 = 14.0;
    const LINE_GAP: f64 = 3.0;
    let (headline_width, headline_height) = headline_layout.pixel_size();
    let (state_width, state_height) = state_layout.pixel_size();
    let text_width = headline_width.max(state_width) as f64;
    let text_height = (headline_height + state_height) as f64 + LINE_GAP;
    let (width, height) = (text_width + 2.0 * PADDING_X, text_height + 2.0 * PADDING_Y);
    let fits_on_right = x + POINTER_GAP + width <= plot.right();
    let left = if fits_on_right { x + POINTER_GAP } else { x - POINTER_GAP - width };
    let (left, top) = (left.round(), (y - height - POINTER_GAP).max(plot.top).round());

    rounded_rectangle(cr, left + 0.5, top + 0.5, width - 1.0, height - 1.0, view.palette.radius);
    set_color(cr, view.palette.background, 1.0);
    cr.fill_preserve()?;
    set_color(cr, ink, 0.4);
    cr.set_line_width(1.0);
    cr.stroke()?;

    set_color(cr, ink, 1.0);
    cr.move_to(left + PADDING_X, top + PADDING_Y);
    pangocairo::functions::show_layout(cr, &headline_layout);
    set_color(cr, ink, 0.62);
    cr.move_to(left + PADDING_X, top + PADDING_Y + headline_height as f64 + LINE_GAP);
    pangocairo::functions::show_layout(cr, &state_layout);
    Ok(())
}

fn power_color(plugged: bool) -> Color {
    if plugged { PLUGGED } else { ON_BATTERY }
}

fn set_color(cr: &cairo::Context, color: Color, opacity: f64) {
    cr.set_source_rgba(color.red, color.green, color.blue, opacity);
}

fn label(area: &gtk::DrawingArea, text: &str) -> pango::Layout {
    let layout = area.create_pango_layout(None);
    layout.set_markup(&format!("<span size='small' font_features='tnum'>{text}</span>"));
    layout
}

fn trace(cr: &cairo::Context, points: &[(f64, f64)]) {
    cr.new_path();
    for &(x, y) in points {
        cr.line_to(x, y);
    }
}

fn rounded_rectangle(cr: &cairo::Context, left: f64, top: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    let radius = radius.min(width / 2.0).min(height / 2.0);
    let (right, bottom) = (left + width, top + height);
    cr.new_sub_path();
    cr.arc(right - radius, top + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(right - radius, bottom - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(left + radius, bottom - radius, radius, FRAC_PI_2, PI);
    cr.arc(left + radius, top + radius, radius, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

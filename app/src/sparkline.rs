use crate::theme::{Color, ON_BATTERY, PLUGGED, Palette};
use crate::timeline::{Activity, Timeline};
use adw::prelude::*;
use gtk::cairo;
use std::cell::RefCell;
use std::rc::Rc;

struct View {
    palette: Palette,
    timeline: Rc<Timeline>,
    day_start: i64,
    day_end: i64,
    now: i64,
}

/// A miniature of one day's battery curve, shown inside that day's button in the week strip.
pub struct Sparkline {
    pub area: gtk::DrawingArea,
    view: Rc<RefCell<View>>,
}

impl Sparkline {
    pub fn new(palette: &Palette) -> Self {
        let area = gtk::DrawingArea::builder().hexpand(true).height_request(26).build();
        let view = Rc::new(RefCell::new(View { palette: palette.clone(), timeline: Rc::default(), day_start: 0, day_end: 1, now: 0 }));
        // The day button underneath handles clicks.
        area.set_can_target(false);
        area.set_draw_func({
            let view = view.clone();
            move |_, cr, width, height| {
                // A cairo error leaves the context unusable; there is nothing to recover mid-frame.
                let _ = draw(cr, width as f64, height as f64, &view.borrow());
            }
        });
        Self { area, view }
    }

    pub fn show_day(&self, timeline: Rc<Timeline>, day_start: i64, day_end: i64, now: i64) {
        let mut view = self.view.borrow_mut();
        view.timeline = timeline;
        view.day_start = day_start;
        view.day_end = day_end;
        view.now = now;
        drop(view);
        self.area.queue_draw();
    }
}

fn draw(cr: &cairo::Context, width: f64, height: f64, view: &View) -> Result<(), cairo::Error> {
    const PADDING: f64 = 2.0;

    let visible_until = view.now.min(view.day_end);
    let x = |ts: i64| width * (ts - view.day_start) as f64 / (view.day_end - view.day_start) as f64;
    let y = |percent: f64| height - PADDING - (height - 2.0 * PADDING) * percent / 100.0;
    let ink = view.palette.foreground;

    let mut stretches = view.timeline.stretches_between(view.day_start, visible_until).peekable();
    if stretches.peek().is_none() {
        set_color(cr, ink, 0.18);
        cr.set_line_width(1.0);
        cr.set_dash(&[1.0, 3.0], 0.0);
        cr.move_to(0.0, height - 1.5);
        cr.line_to(width, height - 1.5);
        return cr.stroke();
    }

    cr.set_line_join(cairo::LineJoin::Round);
    for stretch in stretches {
        let curve: Vec<(f64, f64)> = view
            .timeline
            .battery_curve_between(stretch.start, stretch.end)
            .into_iter()
            .map(|(ts, percent)| (x(ts), y(percent)))
            .collect();
        let (Some(first), Some(last)) = (curve.first(), curve.last()) else {
            continue;
        };

        match stretch.session {
            Some(session) if stretch.activity == Activity::ScreenOn => {
                let color = if view.timeline.sessions[session].plugged { PLUGGED } else { ON_BATTERY };
                trace(cr, &curve);
                cr.line_to(last.0, height);
                cr.line_to(first.0, height);
                cr.close_path();
                let fill = cairo::LinearGradient::new(0.0, PADDING, 0.0, height);
                fill.add_color_stop_rgba(0.0, color.red, color.green, color.blue, 0.32);
                fill.add_color_stop_rgba(1.0, color.red, color.green, color.blue, 0.04);
                cr.set_source(&fill)?;
                cr.fill()?;

                trace(cr, &curve);
                set_color(cr, color, 1.0);
                cr.set_line_width(1.5);
            }
            _ => {
                trace(cr, &curve);
                set_color(cr, ink, 0.28);
                cr.set_line_width(1.0);
            }
        }
        cr.stroke()?;
    }
    Ok(())
}

fn set_color(cr: &cairo::Context, color: Color, opacity: f64) {
    cr.set_source_rgba(color.red, color.green, color.blue, opacity);
}

fn trace(cr: &cairo::Context, points: &[(f64, f64)]) {
    cr.new_path();
    for &(x, y) in points {
        cr.line_to(x, y);
    }
}

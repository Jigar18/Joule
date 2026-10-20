use crate::theme::{Color, Palette};
use adw::prelude::*;
use gtk::cairo;
use std::cell::RefCell;
use std::rc::Rc;

struct View {
    palette: Palette,
    focused: f64,
    total: f64,
    growth: f64,
}

/// A segmented bar for one app: how long it was used, split into the part spent
/// in focus and the part it was only counted for holding a lot of memory.
pub struct Meter {
    pub area: gtk::DrawingArea,
    view: Rc<RefCell<View>>,
}

impl Meter {
    pub fn new(palette: &Palette) -> Self {
        let area = gtk::DrawingArea::builder().hexpand(true).height_request(10).build();
        let view = Rc::new(RefCell::new(View { palette: palette.clone(), focused: 0.0, total: 0.0, growth: 1.0 }));
        // The row underneath handles clicks.
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

    /// Both are fractions of the full bar width, with `focused` never above `total`.
    pub fn set_shares(&self, focused: f64, total: f64) {
        let mut view = self.view.borrow_mut();
        view.focused = focused;
        view.total = total;
        drop(view);
        self.area.queue_draw();
    }

    /// 0 to 1 while the bar grows in from the left.
    pub fn set_growth(&self, growth: f64) {
        self.view.borrow_mut().growth = growth;
        self.area.queue_draw();
    }
}

fn draw(cr: &cairo::Context, width: f64, height: f64, view: &View) -> Result<(), cairo::Error> {
    const SEGMENT_WIDTH: f64 = 3.0;
    const SEGMENT_STEP: f64 = 5.0;

    let segments = ((width + SEGMENT_STEP - SEGMENT_WIDTH) / SEGMENT_STEP).floor() as usize;
    let total = view.total.clamp(0.0, 1.0);
    let focused = view.focused.clamp(0.0, 1.0);

    // Anything above zero keeps at least one segment so a small share never looks like none.
    let segments_for = |share: f64| {
        let share = share * view.growth;
        if share > 0.0 { ((share * segments as f64).round() as usize).max(1) } else { 0 }
    };
    let filled_segments = segments_for(total);
    let focused_segments = segments_for(focused).min(filled_segments);

    for segment in 0..segments {
        let (color, opacity) = if segment < focused_segments {
            (view.palette.accent, 1.0)
        } else if segment < filled_segments {
            (view.palette.accent, 0.5)
        } else {
            (view.palette.foreground, 0.08)
        };
        set_color(cr, color, opacity);
        cr.rectangle(segment as f64 * SEGMENT_STEP, 0.0, SEGMENT_WIDTH, height);
        cr.fill()?;
    }
    Ok(())
}

fn set_color(cr: &cairo::Context, color: Color, opacity: f64) {
    cr.set_source_rgba(color.red, color.green, color.blue, opacity);
}

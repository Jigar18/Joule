use joule_store::{MAX_AWAKE_GAP_SECONDS, Sample};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Activity {
    ScreenOn,
    ScreenOff,
    Asleep,
}

/// A run of time with the same activity inside the same session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stretch {
    pub start: i64,
    pub end: i64,
    pub activity: Activity,
    /// Missing when the charger was connected or removed while the machine slept,
    /// so the time belongs to neither the session before nor the one after.
    pub session: Option<usize>,
}

/// One uninterrupted period of running on the charger, or on the battery.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub start: i64,
    pub end: i64,
    pub plugged: bool,
    pub start_percent: u8,
    pub end_percent: u8,
    pub screen_on_seconds: i64,
}

#[derive(Default)]
pub struct Timeline {
    pub stretches: Vec<Stretch>,
    pub sessions: Vec<Session>,
    battery_curve: Vec<(i64, f64)>,
}

impl Timeline {
    pub fn build(samples: &[Sample]) -> Self {
        let mut timeline = Self::default();
        let Some(first) = samples.first() else {
            return timeline;
        };
        timeline.open_session(first);
        timeline.battery_curve.push(curve_point(first));

        for pair in samples.windows(2) {
            let (earlier, later) = (&pair[0], &pair[1]);
            let asleep = later.ts - earlier.ts > MAX_AWAKE_GAP_SECONDS;
            let recharged_while_asleep = asleep && !earlier.plugged && later.battery_percent > earlier.battery_percent;
            let session_continues = earlier.plugged == later.plugged && !recharged_while_asleep;

            if asleep && !session_continues {
                timeline.add_stretch(earlier.ts, later.ts, Activity::Asleep, None);
                timeline.open_session(later);
            } else {
                let activity = match (asleep, later.screen_on) {
                    (true, _) => Activity::Asleep,
                    (false, true) => Activity::ScreenOn,
                    (false, false) => Activity::ScreenOff,
                };
                let session = timeline.sessions.len() - 1;
                timeline.add_stretch(earlier.ts, later.ts, activity, Some(session));
                timeline.extend_session(later, activity);
                if !session_continues {
                    timeline.open_session(later);
                }
            }

            timeline.extend_battery_curve(earlier, later, asleep || !session_continues);
        }

        if let Some(last) = samples.last() {
            timeline.keep_curve_point(last);
        }
        // A session opened by the very last sample has not lasted any time yet.
        if timeline.sessions.last().is_some_and(|session| session.start == session.end) {
            timeline.sessions.pop();
        }
        timeline
    }

    fn open_session(&mut self, sample: &Sample) {
        self.sessions.push(Session {
            start: sample.ts,
            end: sample.ts,
            plugged: sample.plugged,
            start_percent: sample.battery_percent,
            end_percent: sample.battery_percent,
            screen_on_seconds: 0,
        });
    }

    fn extend_session(&mut self, until: &Sample, activity: Activity) {
        let Some(session) = self.sessions.last_mut() else {
            return;
        };
        if activity == Activity::ScreenOn {
            session.screen_on_seconds += until.ts - session.end;
        }
        session.end = until.ts;
        session.end_percent = until.battery_percent;
    }

    fn add_stretch(&mut self, start: i64, end: i64, activity: Activity, session: Option<usize>) {
        match self.stretches.last_mut() {
            Some(last) if last.activity == activity && last.session == session => last.end = end,
            _ => self.stretches.push(Stretch { start, end, activity, session }),
        }
    }

    /// The battery reading moves in whole percents, which would draw as a staircase.
    /// Keeping only the moments the reading changed lets the curve ramp between them.
    fn extend_battery_curve(&mut self, earlier: &Sample, later: &Sample, at_boundary: bool) {
        if at_boundary {
            self.keep_curve_point(earlier);
            self.keep_curve_point(later);
        } else if self.battery_curve.last().is_some_and(|&(_, percent)| percent != later.battery_percent as f64) {
            self.keep_curve_point(later);
        }
    }

    fn keep_curve_point(&mut self, sample: &Sample) {
        if self.battery_curve.last().is_some_and(|&(ts, _)| ts < sample.ts) {
            self.battery_curve.push(curve_point(sample));
        }
    }

    /// Stretches overlapping `[from, to]`, trimmed to that range.
    pub fn stretches_between(&self, from: i64, to: i64) -> impl Iterator<Item = Stretch> {
        self.stretches
            .iter()
            .filter(move |stretch| stretch.start < to && stretch.end > from)
            .map(move |stretch| Stretch { start: stretch.start.max(from), end: stretch.end.min(to), ..*stretch })
    }

    pub fn stretch_at(&self, ts: i64) -> Option<&Stretch> {
        self.stretches.iter().find(|stretch| stretch.start <= ts && ts < stretch.end)
    }

    /// Sessions that were under way at some point during `[from, to]`.
    pub fn sessions_between(&self, from: i64, to: i64) -> Vec<usize> {
        let overlaps = |session: &Session| session.start < to && session.end > from;
        (0..self.sessions.len()).filter(|&index| overlaps(&self.sessions[index])).collect()
    }

    /// Screen-on time within `[from, to]`, optionally only while plugged in or only on battery.
    pub fn screen_on_seconds(&self, from: i64, to: i64, plugged: Option<bool>) -> i64 {
        self.stretches_between(from, to)
            .filter(|stretch| stretch.activity == Activity::ScreenOn)
            .filter(|stretch| match (plugged, stretch.session) {
                (Some(plugged), Some(session)) => self.sessions[session].plugged == plugged,
                _ => plugged.is_none(),
            })
            .map(|stretch| stretch.end - stretch.start)
            .sum()
    }

    /// Battery percentage lost while running on battery within `[from, to]`.
    pub fn battery_drained(&self, from: i64, to: i64) -> f64 {
        self.sessions
            .iter()
            .filter(|session| !session.plugged)
            .filter_map(|session| {
                let (start, end) = (session.start.max(from), session.end.min(to));
                if start >= end {
                    return None;
                }
                Some(self.battery_percent_at(start)? - self.battery_percent_at(end)?)
            })
            .filter(|&drop| drop > 0.0)
            .sum()
    }

    pub fn battery_percent_at(&self, ts: i64) -> Option<f64> {
        let after = self.battery_curve.partition_point(|&(point_ts, _)| point_ts < ts);
        let &(later_ts, later_percent) = self.battery_curve.get(after)?;
        if later_ts == ts {
            return Some(later_percent);
        }
        let &(earlier_ts, earlier_percent) = self.battery_curve.get(after.checked_sub(1)?)?;
        let progress = (ts - earlier_ts) as f64 / (later_ts - earlier_ts) as f64;
        Some(earlier_percent + (later_percent - earlier_percent) * progress)
    }

    /// The battery curve over `[from, to]`, including its value at both ends.
    pub fn battery_curve_between(&self, from: i64, to: i64) -> Vec<(i64, f64)> {
        let mut curve = Vec::new();
        curve.extend(self.battery_percent_at(from).map(|percent| (from, percent)));
        curve.extend(self.battery_curve.iter().filter(|&&(ts, _)| from < ts && ts < to));
        curve.extend(self.battery_percent_at(to).map(|percent| (to, percent)));
        curve
    }
}

fn curve_point(sample: &Sample) -> (i64, f64) {
    (sample.ts, sample.battery_percent as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(minute: i64, battery_percent: u8, plugged: bool, screen_on: bool) -> Sample {
        Sample { ts: minute * 60, battery_percent, plugged, screen_on }
    }

    #[test]
    fn unplugging_starts_a_new_session() {
        let timeline = Timeline::build(&[
            sample(0, 100, true, true),
            sample(1, 100, true, true),
            sample(2, 100, false, true),
            sample(3, 99, false, true),
            sample(4, 98, false, false),
        ]);

        assert_eq!(timeline.sessions.len(), 2);
        assert_eq!((timeline.sessions[0].start, timeline.sessions[0].end, timeline.sessions[0].plugged), (0, 120, true));
        assert_eq!((timeline.sessions[1].start, timeline.sessions[1].end, timeline.sessions[1].plugged), (120, 240, false));
        assert_eq!((timeline.sessions[1].start_percent, timeline.sessions[1].end_percent), (100, 98));
    }

    #[test]
    fn screen_time_excludes_screen_off_and_sleep() {
        let timeline = Timeline::build(&[
            sample(0, 80, false, true),
            sample(1, 80, false, true),
            sample(2, 80, false, false),
            sample(60, 78, false, true),
            sample(61, 78, false, true),
        ]);

        assert_eq!(timeline.sessions.len(), 1);
        assert_eq!(timeline.sessions[0].screen_on_seconds, 120);
        assert_eq!(timeline.screen_on_seconds(0, 61 * 60, None), 120);
        assert_eq!(timeline.screen_on_seconds(0, 61 * 60, Some(true)), 0);
        assert_eq!(timeline.screen_on_seconds(30, 61 * 60, Some(false)), 90);
        let activities: Vec<Activity> = timeline.stretches.iter().map(|stretch| stretch.activity).collect();
        assert_eq!(activities, [Activity::ScreenOn, Activity::ScreenOff, Activity::Asleep, Activity::ScreenOn]);
    }

    #[test]
    fn charger_change_during_sleep_belongs_to_neither_session() {
        let timeline = Timeline::build(&[
            sample(0, 40, false, true),
            sample(1, 40, false, true),
            sample(120, 90, true, true),
            sample(121, 91, true, true),
        ]);

        assert_eq!(timeline.sessions.len(), 2);
        assert_eq!(timeline.sessions[0].end, 60);
        assert_eq!(timeline.sessions[1].start, 120 * 60);
        assert_eq!(timeline.stretch_at(3600).map(|stretch| (stretch.activity, stretch.session)), Some((Activity::Asleep, None)));
    }

    #[test]
    fn recharging_while_asleep_splits_a_battery_session() {
        let timeline = Timeline::build(&[
            sample(0, 30, false, true),
            sample(1, 30, false, true),
            sample(300, 100, false, true),
            sample(301, 100, false, true),
        ]);

        assert_eq!(timeline.sessions.len(), 2);
        assert_eq!(timeline.sessions[1].start_percent, 100);
    }

    #[test]
    fn battery_curve_ramps_between_changes() {
        let timeline = Timeline::build(&[
            sample(0, 50, false, true),
            sample(1, 50, false, true),
            sample(2, 50, false, true),
            sample(3, 50, false, true),
            sample(4, 48, false, true),
        ]);

        assert_eq!(timeline.battery_percent_at(120), Some(49.0));
        assert_eq!(timeline.battery_percent_at(240), Some(48.0));
        assert_eq!(timeline.battery_percent_at(241), None);
        assert_eq!(timeline.battery_drained(0, 240), 2.0);
    }
}

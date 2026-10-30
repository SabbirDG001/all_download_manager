//! When scheduled downloads may run.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WhenDone {
    #[default]
    Nothing,
    Sleep,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Schedule {
    pub enabled: bool,
    /// Start time, "HH:MM" (24-hour clock).
    pub start: String,
    /// Stop time; without one, scheduled downloads run until they're done.
    pub stop: Option<String>,
    /// Days it runs on, Monday first.
    pub days: [bool; 7],
    /// What the computer does when the scheduled downloads are finished.
    pub when_done: WhenDone,
}

impl Default for Schedule {
    fn default() -> Self {
        Self { enabled: false, start: "02:00".into(), stop: Some("08:00".into()), days: [true; 7], when_done: WhenDone::Nothing }
    }
}

/// "HH:MM" to minutes after midnight.
pub fn parse_time(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

impl Schedule {
    /// Whether the time window is open. `weekday` is 0 for Monday; `minute`
    /// counts from midnight. A window may run past midnight (22:00–06:00):
    /// it belongs to the day it starts on.
    pub fn is_open(&self, weekday: usize, minute: u32) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(start) = parse_time(&self.start) else { return false };
        let today = self.days[weekday % 7];
        let yesterday = self.days[(weekday + 6) % 7];
        match self.stop.as_deref().and_then(parse_time) {
            None => today && minute >= start,
            Some(stop) if stop > start => today && minute >= start && minute < stop,
            Some(stop) if stop < start => (today && minute >= start) || (yesterday && minute < stop),
            Some(_) => false,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if parse_time(&self.start).is_none() {
            return Err("the start time must look like 02:00".into());
        }
        if let Some(stop) = &self.stop {
            if parse_time(stop).is_none() {
                return Err("the stop time must look like 08:00".into());
            }
        }
        if self.enabled && !self.days.iter().any(|d| *d) {
            return Err("choose at least one day for the scheduler".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(start: &str, stop: Option<&str>) -> Schedule {
        Schedule { enabled: true, start: start.into(), stop: stop.map(Into::into), ..Default::default() }
    }

    const MON: usize = 0;
    const TUE: usize = 1;

    #[test]
    fn daytime_window() {
        let s = sched("09:00", Some("17:30"));
        assert!(!s.is_open(MON, 8 * 60 + 59));
        assert!(s.is_open(MON, 9 * 60));
        assert!(s.is_open(MON, 17 * 60 + 29));
        assert!(!s.is_open(MON, 17 * 60 + 30));
    }

    #[test]
    fn overnight_window_belongs_to_its_start_day() {
        let mut s = sched("22:00", Some("06:00"));
        assert!(s.is_open(MON, 23 * 60));
        assert!(s.is_open(TUE, 5 * 60));
        assert!(!s.is_open(TUE, 7 * 60));
        // Only Monday nights: Tuesday 05:00 is still Monday's window, Tuesday 23:00 isn't.
        s.days = [true, false, false, false, false, false, false];
        assert!(s.is_open(TUE, 5 * 60));
        assert!(!s.is_open(TUE, 23 * 60));
    }

    #[test]
    fn without_stop_time_runs_until_midnight() {
        let s = sched("02:00", None);
        assert!(!s.is_open(MON, 60));
        assert!(s.is_open(MON, 23 * 60 + 59));
    }

    #[test]
    fn disabled_or_invalid_never_opens() {
        let mut s = sched("02:00", None);
        s.enabled = false;
        assert!(!s.is_open(MON, 3 * 60));
        assert!(sched("25:00", None).validate().is_err());
        assert!(!sched("bad", None).is_open(MON, 3 * 60));
    }
}

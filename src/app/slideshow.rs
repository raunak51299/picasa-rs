//! Slideshow timing and interval rules.

use std::time::{Duration, Instant};

pub const MIN_INTERVAL: Duration = Duration::from_secs(2);
pub const MAX_INTERVAL: Duration = Duration::from_secs(30);
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(4);

pub struct Slideshow {
    pub interval: Duration,
    next_advance: Instant,
}

impl Slideshow {
    pub fn new(interval: Duration) -> Self {
        let interval = clamp_interval(interval);
        Self {
            interval,
            next_advance: Instant::now() + interval,
        }
    }

    pub fn is_due(&self, now: Instant) -> bool {
        now >= self.next_advance
    }

    pub fn next_advance(&self) -> Instant {
        self.next_advance
    }

    pub fn defer(&mut self, now: Instant) {
        self.next_advance = now + self.interval;
    }

    pub fn set_interval(&mut self, interval: Duration, now: Instant) {
        self.interval = clamp_interval(interval);
        self.defer(now);
    }
}

pub fn clamp_interval(interval: Duration) -> Duration {
    interval.clamp(MIN_INTERVAL, MAX_INTERVAL)
}

/// Next interval after a step in seconds, clamped to the supported range.
pub fn stepped_interval(current: Duration, step_seconds: i64) -> Duration {
    let current = current.as_secs() as i64;
    let seconds = (current + step_seconds)
        .clamp(MIN_INTERVAL.as_secs() as i64, MAX_INTERVAL.as_secs() as i64);
    Duration::from_secs(seconds as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_steps_stay_inside_the_supported_range() {
        assert_eq!(
            stepped_interval(Duration::from_secs(4), 2),
            Duration::from_secs(6)
        );
        assert_eq!(
            stepped_interval(Duration::from_secs(4), -2),
            Duration::from_secs(2)
        );
        assert_eq!(
            stepped_interval(Duration::from_secs(2), -2),
            Duration::from_secs(2)
        );
        assert_eq!(
            stepped_interval(Duration::from_secs(30), 2),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn explicit_intervals_are_clamped() {
        assert_eq!(clamp_interval(Duration::from_millis(100)), MIN_INTERVAL);
        assert_eq!(clamp_interval(Duration::from_secs(120)), MAX_INTERVAL);
        assert_eq!(
            clamp_interval(Duration::from_secs(8)),
            Duration::from_secs(8)
        );
    }

    #[test]
    fn a_slideshow_becomes_due_after_its_interval() {
        let slideshow = Slideshow::new(Duration::from_secs(4));
        assert!(!slideshow.is_due(Instant::now()));
        assert!(slideshow.is_due(Instant::now() + Duration::from_secs(5)));
    }

    #[test]
    fn deferring_pushes_the_next_advance_forward() {
        let mut slideshow = Slideshow::new(Duration::from_secs(4));
        let now = Instant::now();
        slideshow.defer(now);
        assert!(!slideshow.is_due(now + Duration::from_secs(3)));
        assert!(slideshow.is_due(now + Duration::from_secs(5)));
    }
}

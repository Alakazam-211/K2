//! Boot phase timing.
//!
//! Between the listener coming up and `boot_status::set_ready()` the daemon
//! answers 503 to every real route. Each step of that window logs one line
//! with its milliseconds, and the end logs the total with the slowest steps:
//!
//! ```text
//! [boot] workspace unification sweep 412ms
//! [boot] ready in 1840ms (slowest: workspace unification sweep 412ms, ...)
//! ```

use std::time::Instant;

/// How many of the slowest phases the `ready` line names.
pub const SLOWEST_NAMED: usize = 5;

pub struct BootClock {
    started: Instant,
    last: Instant,
    phases: Vec<(&'static str, u128)>,
}

impl BootClock {
    /// Start timing (the listener is up).
    pub fn start() -> Self {
        let now = Instant::now();
        Self { started: now, last: now, phases: Vec::new() }
    }

    /// The phase that just finished, timed from the previous mark.
    pub fn phase(&mut self, name: &'static str) {
        let now = Instant::now();
        let ms = now.saturating_duration_since(self.last).as_millis();
        self.last = now;
        self.phases.push((name, ms));
        k2_core::log_debug!("{}", phase_line(name, ms));
    }

    /// Boot is ready: log the total and return it in milliseconds.
    pub fn ready(self) -> u128 {
        let total = self.started.elapsed().as_millis();
        k2_core::log_debug!("{}", ready_line(total, &self.phases));
        total
    }
}

pub fn phase_line(name: &str, ms: u128) -> String {
    format!("[boot] {name} {ms}ms")
}

/// `[boot] ready in Nms (slowest: a 40ms, b 12ms)`; phases under 1 ms are
/// not named.
pub fn ready_line(total_ms: u128, phases: &[(&'static str, u128)]) -> String {
    let mut slow: Vec<&(&str, u128)> = phases.iter().filter(|(_, ms)| *ms > 0).collect();
    slow.sort_by(|a, b| b.1.cmp(&a.1));
    slow.truncate(SLOWEST_NAMED);
    if slow.is_empty() {
        return format!("[boot] ready in {total_ms}ms");
    }
    let named: Vec<String> = slow.iter().map(|(n, ms)| format!("{n} {ms}ms")).collect();
    format!("[boot] ready in {total_ms}ms (slowest: {})", named.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_line_names_the_phase_and_its_milliseconds() {
        assert_eq!(phase_line("layouts dedup", 37), "[boot] layouts dedup 37ms");
    }

    #[test]
    fn the_ready_line_names_the_slowest_phases_first() {
        let phases = [("a", 3), ("b", 400), ("c", 0), ("d", 12), ("e", 1), ("f", 2), ("g", 90)];
        assert_eq!(
            ready_line(510, &phases),
            "[boot] ready in 510ms (slowest: b 400ms, g 90ms, d 12ms, a 3ms, f 2ms)"
        );
        assert_eq!(ready_line(4, &[("x", 0)]), "[boot] ready in 4ms");
    }

    #[test]
    fn the_clock_records_each_phase_in_order() {
        let mut c = BootClock::start();
        std::thread::sleep(std::time::Duration::from_millis(15));
        c.phase("first");
        c.phase("second");
        assert_eq!(c.phases.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["first", "second"]);
        assert!(c.phases[0].1 >= 15, "first phase {} ms", c.phases[0].1);
        assert!(c.ready() >= 15);
    }
}

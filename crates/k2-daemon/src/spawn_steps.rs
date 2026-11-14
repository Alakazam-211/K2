//! Session spawn step timing.
//!
//! `/cli/sessions/v2/spawn` does several things before the PTY exists: the
//! resume lookup, the scoped token, vault secrets, the folder-trust gate, the
//! login step. A spawn that takes [`LOG_AT_MS`] or longer (`K2_SPAWN_LOG_MS`
//! changes it) logs one line with every step's milliseconds, so the slow one
//! has a name:
//!
//! ```text
//! [spawn] agent=sales session=… total 812ms: resume lookup 3ms, …, login step 700ms, pty spawn 60ms, after spawn 6ms
//! ```

use std::time::Instant;

/// A spawn this slow (ms) or slower logs its steps.
pub const LOG_AT_MS: u128 = 500;

/// [`LOG_AT_MS`], or `K2_SPAWN_LOG_MS` when set (`0` logs every spawn, for a
/// day of numbers). Read once.
pub fn log_at_ms() -> u128 {
    static AT: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    *AT.get_or_init(|| {
        std::env::var("K2_SPAWN_LOG_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(LOG_AT_MS)
    })
}

pub struct SpawnSteps {
    started: Instant,
    last: Instant,
    steps: Vec<(&'static str, u128)>,
}

impl SpawnSteps {
    pub fn start() -> Self {
        let now = Instant::now();
        Self { started: now, last: now, steps: Vec::new() }
    }

    /// The step that just finished, timed from the previous mark.
    pub fn step(&mut self, name: &'static str) {
        let now = Instant::now();
        self.steps.push((name, now.saturating_duration_since(self.last).as_millis()));
        self.last = now;
    }

    /// The spawn is done: log its steps when it was slow. Returns the line
    /// it logged.
    pub fn finish(&self, agent: &str, session: &str) -> Option<String> {
        let total = self.started.elapsed().as_millis();
        if total < log_at_ms() {
            return None;
        }
        let line = slow_spawn_line(agent, session, total, &self.steps);
        k2_core::log_debug!("{line}");
        Some(line)
    }
}

pub fn slow_spawn_line(agent: &str, session: &str, total_ms: u128, steps: &[(&'static str, u128)]) -> String {
    let parts: Vec<String> = steps.iter().map(|(n, ms)| format!("{n} {ms}ms")).collect();
    format!("[spawn] agent={agent} session={session} total {total_ms}ms: {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_lists_every_step_in_order() {
        let l = slow_spawn_line("sales", "s-1", 812, &[("resume lookup", 3), ("login step", 700), ("pty spawn", 60)]);
        assert_eq!(
            l,
            "[spawn] agent=sales session=s-1 total 812ms: resume lookup 3ms, login step 700ms, pty spawn 60ms"
        );
    }

    #[test]
    fn a_fast_spawn_logs_nothing_and_a_slow_one_logs_its_steps() {
        let mut fast = SpawnSteps::start();
        fast.step("resume lookup");
        assert_eq!(fast.finish("a", "s"), None);

        let mut slow = SpawnSteps::start();
        slow.step("resume lookup");
        std::thread::sleep(std::time::Duration::from_millis(LOG_AT_MS as u64 + 20));
        slow.step("login step");
        let line = slow.finish("a", "s").expect("a slow spawn logs");
        assert!(line.contains("resume lookup "), "{line}");
        let login: u128 = line
            .split("login step ")
            .nth(1)
            .and_then(|s| s.split("ms").next())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("no login ms in {line}"));
        assert!(login >= LOG_AT_MS, "{line}");
    }
}

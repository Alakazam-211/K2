//! Token bucket on the host's clock.

#[derive(Debug, Clone)]
pub(crate) struct Bucket {
    /// Thousandths of a token.
    milli: u64,
    last_ms: u64,
}

impl Bucket {
    pub fn full(burst: u64, now_ms: u64) -> Self {
        Self {
            milli: burst.max(1) * 1000,
            last_ms: now_ms,
        }
    }

    /// Take one token, or say how long until one is available.
    pub fn take(&mut self, now_ms: u64, rate_per_s: u64, burst: u64) -> Result<(), u64> {
        let rate = rate_per_s.max(1);
        let cap = burst.max(1) * 1000;
        // a clock that goes backwards refills nothing
        let elapsed = now_ms.saturating_sub(self.last_ms);
        self.milli = (self.milli + elapsed.saturating_mul(rate)).min(cap);
        self.last_ms = self.last_ms.max(now_ms);
        if self.milli >= 1000 {
            self.milli -= 1000;
            Ok(())
        } else {
            Err((1000 - self.milli).div_ceil(rate))
        }
    }

    pub fn is_full(&self, now_ms: u64, rate_per_s: u64, burst: u64) -> bool {
        let refill = now_ms
            .saturating_sub(self.last_ms)
            .saturating_mul(rate_per_s.max(1));
        self.milli + refill >= burst.max(1) * 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trips_and_recovers() {
        let mut b = Bucket::full(10, 0);
        for _ in 0..10 {
            b.take(0, 10, 10).unwrap();
        }
        assert_eq!(b.take(0, 10, 10), Err(100));
        assert_eq!(b.take(50, 10, 10), Err(50));
        b.take(100, 10, 10).unwrap();
        assert_eq!(b.take(100, 10, 10), Err(100));
        assert!(b.is_full(1100, 10, 10));
        // sustained 10/s passes
        for i in 0..50 {
            b.take(2000 + i * 100, 10, 10).unwrap();
        }
    }
}

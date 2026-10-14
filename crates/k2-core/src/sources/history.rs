//! Trend history maths (PRD §5.7): raw points for 48 h, hourly means for
//! 35 days, gap markers (`v = None`), and even downsampling to <= 500.

use std::collections::BTreeMap;

/// Raw points are kept this long (seconds).
pub const RAW_KEEP_SECS: i64 = 48 * 3600;
/// Hourly means are kept this long (seconds).
pub const HOUR_KEEP_SECS: i64 = 35 * 86400;
/// The most points one history answer carries.
pub const MAX_POINTS: usize = 500;

/// `raw` when the range starts inside the raw window, else `hour`.
pub fn resolution(from: i64, now: i64) -> &'static str {
    if from >= now - RAW_KEEP_SECS {
        "raw"
    } else {
        "hour"
    }
}

/// Even downsampling to at most `max` points. Each bucket keeps its first
/// time and the mean of its values; a bucket holding a gap marker is a
/// gap, so a gap never disappears into a mean.
pub fn downsample(points: &[(i64, Option<f64>)], max: usize) -> Vec<(i64, Option<f64>)> {
    if points.len() <= max || max == 0 {
        return points.to_vec();
    }
    let n = points.len();
    let mut out = Vec::with_capacity(max);
    for b in 0..max {
        let lo = b * n / max;
        let hi = ((b + 1) * n / max).max(lo + 1).min(n);
        let bucket = &points[lo..hi];
        let t = bucket[0].0;
        if bucket.iter().any(|(_, v)| v.is_none()) {
            out.push((t, None));
        } else {
            let sum: f64 = bucket.iter().filter_map(|(_, v)| *v).sum();
            out.push((t, Some(sum / bucket.len() as f64)));
        }
    }
    out
}

/// Hourly means of raw points older than `before`, keyed by the hour's
/// start. An hour with only gap markers is a gap.
pub fn hourly_means(raw: &[(i64, Option<f64>)], before: i64) -> Vec<(i64, Option<f64>)> {
    let mut hours: BTreeMap<i64, (f64, usize, bool)> = BTreeMap::new();
    for (t, v) in raw.iter().filter(|(t, _)| *t < before) {
        let h = t.div_euclid(3600) * 3600;
        let e = hours.entry(h).or_insert((0.0, 0, false));
        match v {
            Some(x) => {
                e.0 += x;
                e.1 += 1;
            }
            None => e.2 = true,
        }
    }
    hours.into_iter().map(|(h, (sum, n, _gap))| (h, (n > 0).then(|| sum / n as f64))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_keeps_gaps_and_caps_count() {
        let pts: Vec<(i64, Option<f64>)> =
            (0..2000).map(|i| (i as i64 * 60, if i == 1000 { None } else { Some(i as f64) })).collect();
        let d = downsample(&pts, 500);
        assert_eq!(d.len(), 500);
        assert_eq!(d[0], (0, Some(1.5)));
        assert_eq!(d.iter().filter(|(_, v)| v.is_none()).count(), 1, "the gap survives");
        assert_eq!(downsample(&pts[..10], 500).len(), 10);
    }

    #[test]
    fn hourly_means_and_resolution() {
        let raw = vec![(0, Some(1.0)), (60, Some(3.0)), (3600, None), (7200, Some(5.0)), (9000, Some(7.0))];
        let h = hourly_means(&raw, 9000);
        assert_eq!(h, vec![(0, Some(2.0)), (3600, None), (7200, Some(5.0))]);
        assert_eq!(resolution(1_000_000 - 3600, 1_000_000), "raw");
        assert_eq!(resolution(1_000_000 - RAW_KEEP_SECS - 1, 1_000_000), "hour");
    }
}

//! compute_routes' `--expect` readers (design-compute-duration-aware-queue-v1)
//! and the HTTP status line, moved out as they were (0.46.18 queue, ratchet).

use super::*;

/// `--expect <duration>|unknown` (design-compute-duration-aware-queue-v1):
/// `expectSecs` (1 s–7 d) or `expectUnknown`, never both.
pub(super) fn expect_from(v: &serde_json::Value) -> R<estimate::Said> {
    let unknown = b(v, "expectUnknown") == Some(true);
    match (n(v, "expectSecs"), unknown) {
        (Some(_), true) => Err(no(codes::BAD_REQUEST, 400, "--expect takes a duration or unknown, not both")),
        (Some(x), false) if (1..=estimate::EXPECT_MAX_SECS).contains(&x) => Ok(estimate::Said::Secs(x)),
        (Some(_), false) => Err(no(codes::BAD_REQUEST, 400, "--expect is between 1s and 7d, or unknown on a first run (e.g. --expect 12m)")),
        (None, true) => Ok(estimate::Said::Unknown),
        (None, false) => Ok(estimate::Said::Nothing),
    }
}

/// A job's expected run time from its family's runs (on `node` first when
/// named) and what its submitter said. A pooled read.
pub(super) fn estimate_for(ws: &Ws, argv: &[String], tag: Option<&str>, node: Option<&str>, said: estimate::Said) -> estimate::Estimate {
    let now = compute::now();
    k2_core::db::read(|conn| {
        let any = estimate::family_runs(conn, &ws.id, argv, tag, None, estimate::HISTORY);
        let here = node.map(|n| estimate::family_runs(conn, &ws.id, argv, tag, Some(n), estimate::HISTORY)).unwrap_or_default();
        estimate::pick(&here, &any, said, now)
    })
}

pub(super) fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

//! The Rust CLI's spec verbs against the daemon (R4.0): every spec row's
//! `(method, path)` is a `ROUTES` row with a floor for that method, for
//! the rows of the `k2 --schema` entries and the test fixture's. The rows
//! come through `k2_core::contract::cli_spec`, so k2-daemon gains no
//! k5-cli dependency. A seeded `/cli/nope` fails.

use std::path::PathBuf;

use k2_core::contract::cli_spec::{rows_in, rows_of, SpecRow, FIXTURE_ENTRIES};
use k2_daemon::routes::route_policy::lookup;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture() -> Vec<SpecRow> {
    let text = std::fs::read_to_string(root().join(FIXTURE_ENTRIES))
        .unwrap_or_else(|e| panic!("read {FIXTURE_ENTRIES}: {e}"));
    rows_of(FIXTURE_ENTRIES, &text).unwrap_or_else(|e| panic!("{e}"))
}

/// Problems with the rows' routes against `ROUTES`.
fn route_problems(rows: &[SpecRow]) -> Vec<String> {
    let mut out = Vec::new();
    for r in rows {
        let Some(row) = lookup(&r.path) else {
            out.push(format!(
                "{}: {} {} isn't a ROUTES row",
                r.entry, r.method, r.path
            ));
            continue;
        };
        let floor = match r.method {
            "GET" => row.get,
            other => panic!("{}: a method the runner doesn't send: {other}", r.entry),
        };
        if floor.is_none() {
            out.push(format!(
                "{}: ROUTES has no {} floor for {}",
                r.entry, r.method, r.path
            ));
        }
    }
    out
}

#[test]
fn every_spec_row_is_a_routes_row() {
    let mut rows = rows_in(&root()).unwrap_or_else(|e| panic!("{e}"));
    let fixture = fixture();
    assert!(
        fixture.iter().any(|r| r.path == "/cli/compute/stats"),
        "the fixture is compute stats: {fixture:?}"
    );
    rows.extend(fixture);
    let probs = route_problems(&rows);
    assert!(probs.is_empty(), "{}", probs.join("\n"));
}

#[test]
fn a_spec_row_to_an_unknown_route_fails() {
    let mut rows = fixture();
    rows[0].path = "/cli/nope".into();
    assert_eq!(
        route_problems(&rows),
        ["compute stats: GET /cli/nope isn't a ROUTES row"]
    );
    // A route that serves only POST has no GET floor.
    rows[0].path = "/cli/compute/run".into();
    let probs = route_problems(&rows);
    assert_eq!(
        probs,
        ["compute stats: ROUTES has no GET floor for /cli/compute/run"],
        "{probs:?}"
    );
}

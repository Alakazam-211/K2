#!/usr/bin/env python3
"""audit-ignores.py check — the advisory allow-list rules (prd-security-sweep-0464-v1 SS14).

Two files hold the advisories CI lets through:

  osv-scanner.toml    [[IgnoredVulns]] id / ignoreUntil / reason, read by
                      osv-scanner (bun.lock + Cargo.lock). This file is the
                      ledger: every allowed advisory has its reason and its
                      review-by date (ignoreUntil) here.
  .cargo/audit.toml   [advisories] ignore = [...], read by cargo audit (it
                      looks for .cargo/audit.toml, not a root audit.toml).
                      IDs only; each one must also be in osv-scanner.toml.

Rules, each a failure:
  - every entry has an id, a reason of at least 20 characters, and an
    ignoreUntil date;
  - ignoreUntil is after today (an expired entry fails here, and osv-scanner
    itself stops ignoring it on that date) and at most 90 days out;
  - no id twice;
  - every cargo-audit id is in osv-scanner.toml.

To renew an entry: re-check reachability, update the reason if it changed,
and move ignoreUntil (at most 90 days). To drop one: bump the dependency and
delete the entry from both files.

  python3 scripts/ci/audit-ignores.py check [--today YYYY-MM-DD] [--osv F] [--cargo F]
"""

import argparse
import datetime as dt
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MAX_DAYS = 90
MIN_REASON = 20


def as_date(v):
    if isinstance(v, dt.datetime):
        return v.date()
    if isinstance(v, dt.date):
        return v
    return None


def check(osv_path: Path, cargo_path: Path, today: dt.date) -> list[str]:
    errs: list[str] = []
    osv = tomllib.loads(osv_path.read_text())
    cargo = tomllib.loads(cargo_path.read_text())
    entries = osv.get("IgnoredVulns", [])
    if not isinstance(entries, list):
        return [f"{osv_path.name}: IgnoredVulns must be an array of tables"]
    seen: set[str] = set()
    for n, e in enumerate(entries, 1):
        vid = e.get("id")
        where = f"{osv_path.name} entry {n}" + (f" ({vid})" if vid else "")
        if not isinstance(vid, str) or not vid.strip():
            errs.append(f"{where}: no id")
            continue
        if vid in seen:
            errs.append(f"{where}: listed twice")
        seen.add(vid)
        reason = e.get("reason")
        if not isinstance(reason, str) or len(reason.strip()) < MIN_REASON:
            errs.append(f"{where}: needs a reason (at least {MIN_REASON} characters: why it can't be fixed now and why it isn't reachable)")
        until = as_date(e.get("ignoreUntil"))
        if until is None:
            errs.append(f"{where}: needs ignoreUntil (the review-by date, at most {MAX_DAYS} days out)")
        elif until <= today:
            errs.append(f"{where}: expired on {until}; re-check it and move ignoreUntil, or fix it")
        elif (until - today).days > MAX_DAYS:
            errs.append(f"{where}: ignoreUntil {until} is more than {MAX_DAYS} days out")
    ignore = cargo.get("advisories", {}).get("ignore", [])
    if not isinstance(ignore, list):
        errs.append(f"{cargo_path.name}: advisories.ignore must be a list")
        ignore = []
    cargo_seen: set[str] = set()
    for vid in ignore:
        if vid in cargo_seen:
            errs.append(f"{cargo_path.name}: {vid} listed twice")
        cargo_seen.add(vid)
        if vid not in seen:
            errs.append(f"{cargo_path.name}: {vid} has no entry (reason, ignoreUntil) in {osv_path.name}")
    return errs


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["check"])
    ap.add_argument("--today", type=dt.date.fromisoformat, default=dt.datetime.now(dt.timezone.utc).date())
    ap.add_argument("--osv", type=Path, default=ROOT / "osv-scanner.toml")
    ap.add_argument("--cargo", type=Path, default=ROOT / ".cargo" / "audit.toml")
    a = ap.parse_args()
    errs = check(a.osv, a.cargo, a.today)
    for e in errs:
        print(f"audit-ignores: {e}", file=sys.stderr)
    if errs:
        return 1
    n = len(tomllib.loads(a.osv.read_text()).get("IgnoredVulns", []))
    print(f"audit-ignores: {n} allowed advisories, each with a reason and a review date within {MAX_DAYS} days")
    return 0


if __name__ == "__main__":
    sys.exit(main())

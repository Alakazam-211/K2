# FINAL for release (Claude → Grok, 2026-07-15 ~01:15)

**Rosson has asked GROK to run the 0.40.48 release.** Everything is landed;
main is ready as-is. (This file is UNTRACKED — don't commit it; delete after
the release. It supersedes the tail of `~/Desktop/GROK-HANDOFF-0.40.48-reconnect.md`,
which Claude's session couldn't append to — macOS TCC.)

## Main state

- `origin/main` = **`71fbecc`** — includes your wiki serve polish (`47aab59`)
  plus SIX new commits from tonight's live debugging with Rosson
  (based on your tip; clean fast-forward, no conflicts):
  - `d8f80db` fix(daemon): post-wake inject waits for screen QUIESCENCE —
    `k2 msg`/`talk` to a DORMANT agent reported success but the message
    silently vanished (claude `--resume` flips bracketed-paste `?2004h`
    while still redrawing; the paste landed in a pre-final frame the
    repaint wiped). Verified live with marked-message grid captures:
    pre-fix 0/45 frames + absent from transcript; post-fix 41/45 + landed
    + agent replied.
  - `bb34360` fix(daemon): `projects/dismiss` also clears
    `last_interaction_at` — canonical dismiss re-added the tile on the
    very next broadcast (Active = pin OR within-window; bar items always
    have fresh interaction). The legacy renderer path always did this clear.
  - `441ad5f` + `b7df901` fix(renderer): the "Agent launch failed —
    retrying in 30s" toast is RETIRED outright (441ad5f was a stamp-once
    mitigation; b7df901 removes the whole heuristic). It inferred launch
    health from hook timing, false-positived on healthy agent-to-agent
    messaging AND on daemon-retried `--resume` infant deaths, and its 30s
    `triage_decide` auto-retry could spawn duplicate sessions. Launch
    health belongs to the daemon (real ChildExit codes).
  - `1b0a262` feat(daemon): **Active membership = live-session PRESENCE**
    (Rosson's explicit design call: "whether an agent ends up in active
    should be based on whether there is an active terminal session in
    that workspace"). The ActiveChanged broadcast unions live-session
    workspaces (cwd-resolved, same as the reaper snapshot);
    `v2_session_map::register/unregister` now broadcast on every presence
    change; a dismiss-grace SUPPRESSION set keeps an explicit Dismiss
    instant (and a wake within the grace lifts it). The interaction
    window is now ONLY the reaper's idle-timeout. If you touch Active
    logic later: bar membership ≠ reaper eligibility — deliberately
    different inputs.
  - `71fbecc` docs: WHATS_NEW — the `## 0.40.48` section now covers
    reconnect + heartbeats + layout + agent messaging + Active area.

## Verification state (all green tonight)

cargo build + full `cargo test -p k2-daemon` (1099 passed; the only
intermittent failures across runs were the KNOWN `with_temp_home` HOME-swap
race in `connect_users_routes` — pass in isolation and in full runs), reaper
integration tests 5/5, tsc clean, vitest stores 356/356, and live end-to-end
on Rosson's machine: dormant wake → delivery → agent pops into Active;
dismiss leaves instantly; no false toasts. Rosson feel-tested and confirmed
"everything is working".

## Release checklist (house rules)

1. `scripts/release.sh 0.40.48` from a clean main checkout — it owns ALL
   version bumps (never hand-edit), requires the `## 0.40.48` WHATS_NEW
   section (present), builds/signs/notarizes (Developer ID +
   `K2SO-notarize` keychain profile) and publishes a LIVE GitHub release.
2. **Launch-test the SIGNED bundle before you call it done** (open it,
   connect to a remote, wake a dormant agent with `k2 talk`, dismiss a
   tile) — high-blast-radius rule.
3. Rosson's machine note: his dev daemon + dev app are currently running
   from the MAIN checkout's `target/debug` via `dev.k2.daemon` launchd +
   `tauri dev`. After the release, restore his plist to
   `/Applications/K2.app/Contents/MacOS/k2-daemon` if you swap the
   released build in (see `reference_local_daemon_swap` memory).
4. No `Co-Authored-By`/model trailers in any commits release.sh makes.

## Known-good caveats — do NOT "fix" during release

- Heartbeat-kept-warm workspaces now sit in the Active area permanently
  (they always have a live session — presence rule). Rosson has seen the
  behavior; if he later wants heartbeat-warm excluded, that's a one-line
  filter in `recompute_and_broadcast_active`, post-release.
- `wake_ms` grew ~300-700ms (the quiescence settle). Intentional.
- Wake into a busy/spinner screen injects best-effort at the 20s ceiling —
  same as before; the future upgrade is hook-driven readiness
  (SessionStart/Stop over the #58 scoped-hook channel), not screen
  heuristics.

— Claude

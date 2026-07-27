// Unit tests for the ConnectionGate acceptance POLICIES (K2 Connect
// step #4). The gate component itself is React/Tauri-bound; these tests
// pin the version/protocol decision logic that the policies encapsulate,
// which is the part the PRD requires to differ for local vs remote.
//
//   - localPairedPolicy: exact version-string match (auto-update guard).
//   - remoteHostPolicy:  protocol range-check, NO version-string match
//     (a remote daemon may run a different marketing version).

import { describe, it, expect } from 'vitest'
import {
  localPairedPolicy,
  remoteHostPolicy,
  shouldSurfaceRemoteDrop,
  classifyWhoamiStatus,
  shouldRefreshCredsOnAccept,
  shouldShowReloadButton,
  isWedgePatternEstablished,
  arbiterProvesHostReady,
  advanceWedgeFailureClock,
  isFlapPatternEstablished,
  pruneFlapTimestamps,
  WEDGE_PATTERN_MS,
  WEDGE_CLEAR_OK_STREAK,
  WEDGE_FLAP_THRESHOLD,
  WEDGE_FLAP_WINDOW_MS,
} from './ConnectionGate'

const status = (over: Partial<{ version: string; protocol: number; phase: string; detail: string }> = {}) => ({
  version: '0.39.15',
  protocol: 1,
  phase: 'ready',
  detail: '',
  ...over,
})

describe('localPairedPolicy', () => {
  it('accepts the exact-version, ready daemon', () => {
    const p = localPairedPolicy('0.39.15')
    expect(p.decide(status()).kind).toBe('accept')
  })

  it('waits on a version mismatch (outgoing old daemon mid-update)', () => {
    const p = localPairedPolicy('0.39.15')
    expect(p.decide(status({ version: '0.39.14' })).kind).toBe('wait')
  })

  it('shows migrating when the right version is not yet ready', () => {
    const p = localPairedPolicy('0.39.15')
    const d = p.decide(status({ phase: 'migrating', detail: 'x' }))
    expect(d.kind).toBe('migrating')
  })

  it('waits when unreachable (null status)', () => {
    expect(localPairedPolicy('0.39.15').decide(null).kind).toBe('wait')
  })
})

describe('remoteHostPolicy', () => {
  it('accepts a DIFFERENT marketing version when protocol is compatible + ready', () => {
    const p = remoteHostPolicy()
    // Remote daemon on a totally different version string — still accepted.
    expect(p.decide(status({ version: '1.2.3-hosted', protocol: 1 })).kind).toBe('accept')
  })

  it('accepts a higher (forward-compatible) protocol', () => {
    expect(remoteHostPolicy().decide(status({ protocol: 5 })).kind).toBe('accept')
  })

  it('waits when the remote protocol is below the minimum', () => {
    expect(remoteHostPolicy().decide(status({ protocol: 0 })).kind).toBe('wait')
  })

  it('shows migrating when ready phase not reached', () => {
    expect(remoteHostPolicy().decide(status({ phase: 'starting' })).kind).toBe('migrating')
  })

  it('waits when unreachable (null status)', () => {
    expect(remoteHostPolicy().decide(null).kind).toBe('wait')
  })

  it('does NOT require version-string equality (the local guard must not leak in)', () => {
    // Same status object that localPairedPolicy('X') would REJECT on
    // version is accepted by the remote policy.
    const local = localPairedPolicy('expected-version')
    const remote = remoteHostPolicy()
    const s = status({ version: 'something-else' })
    expect(local.decide(s).kind).toBe('wait')
    expect(remote.decide(s).kind).toBe('accept')
  })
})

// K2 Connect step #4 — DEBOUNCE the drop. A single slow/blipped health-poll
// over a higher-latency tunnel must NOT surface the reconnect banner while
// the data WS is still streaming; only >= REMOTE_DROP_THRESHOLD (2)
// CONSECUTIVE failed polls count as a genuine drop. This pins the threshold
// rule the gate's poll loop uses (N-1 fails → no banner; Nth → banner).
describe('shouldSurfaceRemoteDrop (debounced drop)', () => {
  it('a single blip does NOT surface the banner (N-1 = 1 fail)', () => {
    expect(shouldSurfaceRemoteDrop(1)).toBe(false)
  })

  it('the threshold-th consecutive fail surfaces the banner (N = 2)', () => {
    expect(shouldSurfaceRemoteDrop(2)).toBe(true)
  })

  it('stays surfaced past the threshold', () => {
    expect(shouldSurfaceRemoteDrop(3)).toBe(true)
    expect(shouldSurfaceRemoteDrop(10)).toBe(true)
  })

  it('zero fails (just connected / recovered) is never a drop', () => {
    expect(shouldSurfaceRemoteDrop(0)).toBe(false)
  })
})

// 0.39.36 — stale connect-session reconnect fix. Connect-user sessions are
// in-memory in the daemon, so a host restart wipes them while /boot-status
// still answers 200 'ready'. After the policy accepts a token-bearing
// REMOTE host, the gate probes the session with whoami and acts on this
// classification: an authoritative 401/403 ⇒ dead (expire + re-auth); a
// transport blip (null) or a non-auth server hiccup ⇒ 'unknown' (do NOT
// nuke a good session — mount and let the normal path sort it out).
describe('classifyWhoamiStatus (stale-session probe)', () => {
  it('treats 403 as a DEAD session (wiped by a host restart)', () => {
    expect(classifyWhoamiStatus(403)).toBe('dead')
  })

  it('treats 401 as a DEAD session (expired/unauthorized)', () => {
    expect(classifyWhoamiStatus(401)).toBe('dead')
  })

  it('treats 200 as an ALIVE session (mount)', () => {
    expect(classifyWhoamiStatus(200)).toBe('alive')
  })

  it('treats other 2xx as alive', () => {
    expect(classifyWhoamiStatus(204)).toBe('alive')
  })

  it('treats a transport error/timeout (null) as UNKNOWN — never expires', () => {
    // A network blip must NOT nuke a good session — proceed to mount.
    expect(classifyWhoamiStatus(null)).toBe('unknown')
  })

  it('treats a non-auth server hiccup (5xx/404) as UNKNOWN — not an authoritative dead token', () => {
    expect(classifyWhoamiStatus(500)).toBe('unknown')
    expect(classifyWhoamiStatus(502)).toBe('unknown')
    expect(classifyWhoamiStatus(404)).toBe('unknown')
  })
})

// Black-screen-after-self-update fix B — stale cached daemon creds. The
// local daemon rotates its auth token every boot and reuses the port, and
// /boot-status is public: a LOCAL accept that FOLLOWS any non-accept poll
// in the same gate session crossed a daemon restart (upgrade kickstart),
// so the cached port/token MUST be invalidated before App mounts, or every
// authed call 401s. This pins the rule the poll loop's accept branch uses
// to decide whether to call invalidateDaemonWs().
describe('shouldRefreshCredsOnAccept (upgrade-kickstart stale creds)', () => {
  it('LOCAL accept after a non-accept poll → refresh (crossed a daemon restart)', () => {
    expect(shouldRefreshCredsOnAccept({ isRemote: false, sawNonAccept: true })).toBe(true)
  })

  it('LOCAL first-poll accept (no restart crossed) → no refresh', () => {
    expect(shouldRefreshCredsOnAccept({ isRemote: false, sawNonAccept: false })).toBe(false)
  })

  it('REMOTE hosts never refresh here (their token lives in the connect-host store)', () => {
    expect(shouldRefreshCredsOnAccept({ isRemote: true, sawNonAccept: true })).toBe(false)
    expect(shouldRefreshCredsOnAccept({ isRemote: true, sawNonAccept: false })).toBe(false)
  })
})

// Black-screen-after-self-update fix C — a failed Phase-2 App import must
// never strand the user. On a local accept the poll loop stops, freezing
// `attempts`, so the attempt-based thresholds can never fire on their own;
// importFailed must surface the Reload button unconditionally.
describe('shouldShowReloadButton (overlay escape hatch)', () => {
  it('importFailed ALWAYS shows Reload — even with attempts frozen at 0', () => {
    expect(shouldShowReloadButton({ migrating: false, attempts: 0, importFailed: true })).toBe(true)
    expect(shouldShowReloadButton({ migrating: true, attempts: 0, importFailed: true })).toBe(true)
  })

  it('plain wait surfaces Reload after ~10s (attempts >= 20)', () => {
    expect(shouldShowReloadButton({ migrating: false, attempts: 19, importFailed: false })).toBe(false)
    expect(shouldShowReloadButton({ migrating: false, attempts: 20, importFailed: false })).toBe(true)
  })

  it('migrating waits longer (~60s, attempts >= 120) before nagging', () => {
    expect(shouldShowReloadButton({ migrating: true, attempts: 119, importFailed: false })).toBe(false)
    expect(shouldShowReloadButton({ migrating: true, attempts: 120, importFailed: false })).toBe(true)
  })
})

// 0.40.48 — the wedge detector's pure rules. The incident signature is a
// SUSTAINED run of webview boot-probe failures while the arbiter still
// sees the host ready. 0.40.48 originally tracked only {kind:'http'};
// 0.40.68 / GH#57 generalizes to http OR network (TLS handshake eof
// flaps) and adds flap + ok-streak helpers. These rules decide (a) when
// the run is long enough to consult the out-of-webview arbiter and (b)
// whether the arbiter's answer PROVES the host healthy (⇒ webview path
// is poisoned / thrashing).
describe('isWedgePatternEstablished (consecutive webview-failure clock)', () => {
  const t0 = 1_750_000_000_000

  it('no failure run in progress → never established', () => {
    expect(isWedgePatternEstablished({ failingSince: null, now: t0 })).toBe(false)
    // Back-compat alias from 0.40.48:
    expect(isWedgePatternEstablished({ httpFailingSince: null, now: t0 })).toBe(false)
  })

  it('a run younger than the threshold is not yet a wedge', () => {
    expect(
      isWedgePatternEstablished({ failingSince: t0, now: t0 + WEDGE_PATTERN_MS - 1 }),
    ).toBe(false)
  })

  it('a run at/past the threshold is established', () => {
    expect(
      isWedgePatternEstablished({ failingSince: t0, now: t0 + WEDGE_PATTERN_MS }),
    ).toBe(true)
    expect(
      isWedgePatternEstablished({ httpFailingSince: t0, now: t0 + WEDGE_PATTERN_MS * 5 }),
    ).toBe(true)
  })
})

describe('advanceWedgeFailureClock (0.40.68 ok-streak clear)', () => {
  const t0 = 1_750_000_000_000

  it('non-ok starts the failure clock and zeros the ok streak', () => {
    expect(
      advanceWedgeFailureClock({ probeOk: false, failingSince: null, okStreak: 3, now: t0 }),
    ).toEqual({ failingSince: t0, okStreak: 0 })
    expect(
      advanceWedgeFailureClock({
        probeOk: false,
        failingSince: t0 - 1000,
        okStreak: 1,
        now: t0,
      }),
    ).toEqual({ failingSince: t0 - 1000, okStreak: 0 })
  })

  it('a single ok does NOT clear failingSince (GH#57 split-second healthy)', () => {
    const mid = advanceWedgeFailureClock({
      probeOk: true,
      failingSince: t0,
      okStreak: 0,
      now: t0 + 1000,
    })
    expect(mid.failingSince).toBe(t0)
    expect(mid.okStreak).toBe(1)
    expect(mid.okStreak).toBeLessThan(WEDGE_CLEAR_OK_STREAK)
  })

  it('WEDGE_CLEAR_OK_STREAK consecutive oks clear the failure clock', () => {
    let state = { failingSince: t0 as number | null, okStreak: 0 }
    for (let i = 0; i < WEDGE_CLEAR_OK_STREAK; i++) {
      state = advanceWedgeFailureClock({
        probeOk: true,
        failingSince: state.failingSince,
        okStreak: state.okStreak,
        now: t0 + i,
      })
    }
    expect(state).toEqual({ failingSince: null, okStreak: 0 })
  })
})

describe('isFlapPatternEstablished (0.40.68 / GH#57 thrash)', () => {
  const t0 = 1_750_000_000_000

  it('below threshold → not established', () => {
    expect(
      isFlapPatternEstablished({
        reconnectSurfacedAt: [t0, t0 + 1000, t0 + 2000],
        now: t0 + 3000,
      }),
    ).toBe(false)
  })

  it('threshold surfaces inside the window → established', () => {
    const stamps = Array.from({ length: WEDGE_FLAP_THRESHOLD }, (_, i) => t0 + i * 1000)
    expect(isFlapPatternEstablished({ reconnectSurfacedAt: stamps, now: t0 + 10_000 })).toBe(true)
  })

  it('old stamps outside the window do not count', () => {
    const stamps = Array.from(
      { length: WEDGE_FLAP_THRESHOLD },
      (_, i) => t0 - WEDGE_FLAP_WINDOW_MS - 10_000 + i * 100,
    )
    expect(isFlapPatternEstablished({ reconnectSurfacedAt: stamps, now: t0 })).toBe(false)
  })

  it('pruneFlapTimestamps drops aged entries', () => {
    expect(
      pruneFlapTimestamps([t0 - WEDGE_FLAP_WINDOW_MS - 1, t0 - 1000, t0], t0),
    ).toEqual([t0 - 1000, t0])
  })
})

describe('arbiterProvesHostReady (out-of-webview second opinion)', () => {
  const ready = JSON.stringify({ version: '0.40.48', protocol: 1, phase: 'ready', detail: '' })

  it('2xx + phase ready → proven (the poisoned-pool verdict)', () => {
    expect(arbiterProvesHostReady({ status: 200, body: ready })).toBe(true)
  })

  it('the arbiter failing at the network level (null) is a genuine outage, NOT a wedge', () => {
    expect(arbiterProvesHostReady(null)).toBe(false)
  })

  it('a non-2xx from the arbiter (edge 404 for everyone) is not proof', () => {
    expect(arbiterProvesHostReady({ status: 404, body: 'no route found' })).toBe(false)
    expect(arbiterProvesHostReady({ status: 502, body: '' })).toBe(false)
  })

  it("a pre-'ready' phase means the host is genuinely still booting", () => {
    expect(
      arbiterProvesHostReady({
        status: 200,
        body: JSON.stringify({ version: 'x', protocol: 1, phase: 'migrating', detail: 'db' }),
      }),
    ).toBe(false)
  })

  it('an unparseable / non-boot-status body is not proof', () => {
    expect(arbiterProvesHostReady({ status: 200, body: 'not json' })).toBe(false)
    expect(arbiterProvesHostReady({ status: 200, body: JSON.stringify({ nope: 1 }) })).toBe(false)
  })
})

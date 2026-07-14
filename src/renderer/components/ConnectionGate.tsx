/**
 * ConnectionGate — gates BOTH render AND module import of the app on the
 * daemon being (a) the build paired with THIS app and (b) finished with
 * its first-boot migrations.
 *
 * ## Why this exists
 *
 * 0.39.2/0.39.3 deferred the import of <App /> until the daemon answered
 * /ping, so its eager store fetches wouldn't fire against a down daemon.
 * But /ping only proves *something* is listening — and during a
 * 0.38.x → 0.39.x auto-update the OUTGOING old daemon was still bound to
 * the stable port answering /ping while the new daemon was being
 * kickstarted and grinding through its (heavy, one-time) first-boot
 * migration. The gate took that false-positive ping, mounted the app,
 * and its fetches hit the gap where the old daemon had been killed and
 * the new one wasn't serving yet → blank window ("appears to have
 * crashed").
 *
 * ## The fix (0.39.5)
 *
 * The daemon now binds its port BEFORE migrating and exposes a versioned
 * readiness handshake at GET /boot-status:
 *
 *     { version, protocol, phase, detail }
 *
 * This gate polls /boot-status and only mounts when an **acceptance
 * policy** says so. The local/auto-update policy ([`localPairedPolicy`])
 * requires `version === this app's bundled version` AND `phase ===
 * 'ready'` — so it can never bind to the outgoing old daemon (which
 * either reports an older version or, pre-0.39.5, 404s /boot-status
 * entirely), and it can render the migration progress (`detail`) to the
 * user instead of a blank window.
 *
 * ## Future-proofing (K2 Connect)
 *
 * The gate core is version-agnostic: the version/protocol decision lives
 * entirely in the injected policy. K2 Connect, which legitimately talks
 * to a remote daemon of a *different* marketing version, will supply a
 * different policy that range-checks `protocol` instead of requiring
 * exact `version` equality — without touching this component. Keep that
 * logic in the policy, never inline. See
 * [[project_daemon_handshake_contract]].
 */
import React, { useEffect, useRef, useState } from 'react'
import { getDaemonWs, invalidateDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { useConnectHostStore, activeHostKey, type ConnectHost } from '@/stores/connect-host'
import { reviveRemoteSession } from '@/lib/remote-session'
import {
  deriveRecovery,
  recoveryPollMs,
  recoveryStatusText,
  type RemoteRecoveryState,
} from '@/lib/remote-recovery'
import { jittered } from '@/lib/backoff'
import { RemoteSignIn } from './RemoteSignIn'
import { AppErrorBoundary } from './AppErrorBoundary'

/** Shape of the daemon's GET /boot-status response. `detail` is free-text
 *  for the UI only — never branch on it. `instanceId` (0.40.48, optional —
 *  older daemons omit it) is a per-boot random id: a CHANGE between two
 *  accepted polls proves the daemon restarted even when the drop window
 *  fell between health ticks. */
interface DaemonBootStatus {
  version: string
  protocol: number
  phase: string // 'starting' | 'migrating' | 'ready' | 'error' | future
  detail: string
  instanceId?: string
}

/**
 * Lowest daemon PROTOCOL this client can talk to. The daemon ships
 * `boot_status.rs` PROTOCOL = 1; a remote may run a different MARKETING
 * version, so the remote policy range-checks protocol instead of an
 * exact version-string match. Bump this only on a breaking wire change.
 */
const MIN_COMPATIBLE_PROTOCOL = 1

/** Soft-reconnect cadence (K2 Connect step #4). After a remote host
 *  connects, poll its health at this interval so a tunnel drop is
 *  detected and surfaced as the dimmed overlay. */
const REMOTE_HEALTH_POLL_MS = 4000
/** How many CONSECUTIVE failed health-polls a connected remote must rack
 *  up before we call it a real drop. A single slow/blipped poll over a
 *  higher-latency tunnel must NOT trip the reconnect indicator while the
 *  data WS is still streaming ("says dropped but the screen is moving").
 *  Below this threshold the gate stays 'connected' with no banner. */
const REMOTE_DROP_THRESHOLD = 2
/** Per-poll /boot-status timeout for a REMOTE health-poll. Looser than
 *  the local 2s because a tunnel round-trip is inherently higher-latency;
 *  a tight 2s would flag transient slowness as a drop. */
const REMOTE_BOOT_STATUS_TIMEOUT_MS = 4000
/** Timeout for the REMOTE connect-session validity probe (whoami). Short
 *  + bounded so a network hiccup can't hang the gate; on a NON-403
 *  transport error we treat the session as 'unknown' (a blip, not dead)
 *  and proceed to mount rather than nuking a good session. */
const REMOTE_WHOAMI_TIMEOUT_MS = 4000

/** Issue #4 (never-strand): on the FIRST connect with a remembered token we
 *  must POSITIVELY confirm the session is alive before mounting. A remote
 *  update restarts the daemon and wipes its in-memory connect sessions, so a
 *  remembered token can be DEAD even though /boot-status accepts. We retry the
 *  whoami probe a few times so a real network blip resolves to 'alive'; if it
 *  never confirms ('dead' or persistent 'unknown' — e.g. an authenticated probe
 *  timing out through the E2E TLS splice), we drop to RemoteSignIn instead of
 *  mounting a dead token, which otherwise hangs forever on "Connecting…". */
const FIRST_CONNECT_PROBE_RETRIES = 3
const FIRST_CONNECT_PROBE_RETRY_DELAY_MS = 1000

/** Debounce rule for a connected-remote drop: given the running count of
 *  CONSECUTIVE failed health-polls, should we surface the reconnect banner
 *  yet? Pulled out as a pure fn so the threshold behaviour (N-1 fails →
 *  no banner; Nth → banner) is unit-testable without the React/Tauri-bound
 *  effect. Used by the poll loop below. */
export function shouldSurfaceRemoteDrop(consecutiveFails: number): boolean {
  return consecutiveFails >= REMOTE_DROP_THRESHOLD
}

/** Stale-creds rule for the upgrade kickstart (black-screen bug, fix B).
 *  The LOCAL daemon rotates its auth token every boot and reuses the same
 *  port, and /boot-status is PUBLIC — so an accept that FOLLOWS any
 *  non-accept poll in this gate session means we crossed a daemon
 *  restart/upgrade while polling, and daemon-ws's cached creds still hold
 *  the pre-restart token. Mounting on them puts App into a 401 storm.
 *  Pure fn (like shouldSurfaceRemoteDrop above) so the rule is
 *  unit-testable without the React/Tauri-bound effect. Remote hosts are
 *  excluded: their token lives in the connect-host store, not the local
 *  port file, and remote re-auth has its own revival path. */
export function shouldRefreshCredsOnAccept(opts: {
  isRemote: boolean
  sawNonAccept: boolean
}): boolean {
  return !opts.isRemote && opts.sawNonAccept
}

/** Backoff schedule for retrying the Phase-2 dynamic import of App
 *  (fix C). A failed/hung chunk load (e.g. the webview recovered mid-boot
 *  and a stale asset request died) used to be console.error'd and then
 *  NOTHING — a dark "Connecting…" forever. Three retries, easing off. */
const APP_IMPORT_RETRY_DELAYS_MS = [1000, 3000, 9000]

/** Reload-button rule for the ConnectingOverlay, pulled out as a pure fn
 *  for unit tests (fix C). A failed App import must ALWAYS surface the
 *  Reload escape hatch: on a local accept the poll loop stops, so the
 *  attempts counter freezes and the attempt-based thresholds below can
 *  never fire — without the importFailed override the user is stranded. */
export function shouldShowReloadButton(opts: {
  migrating: boolean
  attempts: number
  importFailed: boolean
}): boolean {
  if (opts.importFailed) return true
  return opts.migrating ? opts.attempts >= 120 : opts.attempts >= 20
}

/** The gate's verdict for a single poll. */
type GateDecision =
  | { kind: 'accept' }
  | { kind: 'migrating'; detail: string }
  | { kind: 'wait'; reason: string } // unreachable / wrong version / old daemon

/** Decides whether to mount the app against a daemon, given its
 *  /boot-status (or null when unreachable / 404 / unparseable). */
interface AcceptancePolicy {
  decide(status: DaemonBootStatus | null): GateDecision
}

/**
 * Local auto-update / startup policy: only accept the daemon BUILT AND
 * SHIPPED with this app. `expectedVersion` is the app's bundled version
 * (from Tauri's getVersion()); the release script keeps it in lockstep
 * with the daemon's CARGO_PKG_VERSION, so exact equality is the correct
 * pairing check.
 *
 * If `expectedVersion` is null (non-Tauri/dev context where getVersion()
 * is unavailable) we fall back to readiness-only — still safe, because a
 * pre-0.39.5 daemon has no /boot-status and surfaces as `null` → wait.
 *
 * NOTE: this exact-equality is deliberately confined here. K2 Connect
 * must NOT reuse it — a remote daemon can be a different version. See the
 * file header.
 */
export function localPairedPolicy(expectedVersion: string | null): AcceptancePolicy {
  return {
    decide(status: DaemonBootStatus | null): GateDecision {
      if (!status) {
        // Unreachable, or a pre-0.39.5 daemon that 404s /boot-status —
        // i.e. the outgoing old daemon during an update. Keep waiting.
        return { kind: 'wait', reason: 'unreachable-or-legacy-daemon' }
      }
      if (expectedVersion && status.version !== expectedVersion) {
        // A daemon answered, but it's not the one paired with this app
        // (e.g. an older daemon still up mid-update). Never mount against
        // it — wait for the kickstarted, correctly-versioned daemon.
        //
        // Dev builds tolerate the skew instead: the Rust side's
        // version-check deliberately leaves the installed daemon
        // untouched in dev ("tolerating skew"), so exact equality here
        // would gate a `tauri dev` app off a perfectly reachable daemon
        // forever. Protocol compatibility still applies via /boot-status
        // shape; releases keep the strict pairing check.
        if (import.meta.env.DEV && !import.meta.env.TEST) {
          // eslint-disable-next-line no-console
          console.warn(
            `[connection-gate] DEV: tolerating daemon version skew ` +
              `(daemon ${status.version}, app ${expectedVersion})`,
          )
        } else {
          return { kind: 'wait', reason: `version ${status.version} != app ${expectedVersion}` }
        }
      }
      if (status.phase !== 'ready') {
        // Correct daemon, still finishing first-boot migrations. Show the
        // user what's happening instead of a blank screen.
        return { kind: 'migrating', detail: status.detail }
      }
      return { kind: 'accept' }
    },
  }
}

/**
 * Remote (K2 Connect step #4) policy: a hosted/self-hosted daemon
 * legitimately runs a DIFFERENT marketing version than this app, so we
 * must NOT require version-string equality (that's localPairedPolicy's
 * auto-update guard, scoped to 'local'). Instead range-check the wire
 * `protocol` (>= MIN_COMPATIBLE_PROTOCOL) AND require `phase === 'ready'`.
 *
 * A daemon too old to speak our protocol (or one with no /boot-status →
 * null) keeps the gate waiting rather than mounting against an
 * incompatible wire format.
 */
export function remoteHostPolicy(): AcceptancePolicy {
  return {
    decide(status: DaemonBootStatus | null): GateDecision {
      if (!status) {
        // Unreachable / no /boot-status route — keep retrying (the
        // soft-reconnect overlay surfaces this for a remote host).
        return { kind: 'wait', reason: 'remote-unreachable' }
      }
      if (status.protocol < MIN_COMPATIBLE_PROTOCOL) {
        return {
          kind: 'wait',
          reason: `remote protocol ${status.protocol} < min ${MIN_COMPATIBLE_PROTOCOL}`,
        }
      }
      if (status.phase !== 'ready') {
        return { kind: 'migrating', detail: status.detail }
      }
      return { kind: 'accept' }
    },
  }
}

/** Resolve this app's bundled version via Tauri. Returns null outside a
 *  Tauri context (e.g. a plain browser dev server) so the gate degrades
 *  to readiness-only rather than hanging. */
async function getAppVersion(): Promise<string | null> {
  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    return await getVersion()
  } catch {
    return null
  }
}

/**
 * The boot probe's outcome, DISCRIMINATED (0.40.48). The old shape — null
 * for both a non-2xx response AND a network error — conflated the two
 * failure modes the wedge detector must tell apart:
 *
 *   - 'http'    → the transport worked but the response was an HTTP error.
 *     A genuine reboot barely produces these; a POISONED pooled WKWebView
 *     connection produces them FOREVER (every request rides the dead
 *     tunnel-edge route and 404s "no route found" while the socket itself
 *     stays healthy, so the pool never evicts it).
 *   - 'network' → fetch threw (refused / DNS / timeout) — the ordinary
 *     down/mid-restart signal; the throw itself evicts the dead socket.
 *
 * Every policy call site folds non-'ok' back to the old null, so gate
 * behavior is unchanged outside the wedge detector.
 */
export type BootProbeResult =
  | { kind: 'ok'; status: DaemonBootStatus }
  | { kind: 'http'; httpStatus: number }
  | { kind: 'network' }

/** Hit the daemon's /boot-status with a per-attempt timeout. Returns the
 *  discriminated {@link BootProbeResult}; both failure kinds invalidate the
 *  cached creds so the next poll re-reads them (covers a kickstart-assigned
 *  port change / pre-0.39.5 daemon's 404). `timeoutMs` is looser for a
 *  remote health-poll than for a local boot (a tunnel round-trip is
 *  higher-latency — see REMOTE_BOOT_STATUS_TIMEOUT_MS). Exported for the
 *  unit tests (the http-vs-network discrimination is what the wedge
 *  detector keys on). */
export async function fetchBootStatus(timeoutMs = 2000): Promise<BootProbeResult> {
  try {
    // Host-aware (K2 Connect step #1): polls the ACTIVE host's
    // /boot-status. For 'local' this is byte-identical to before
    // (host === '127.0.0.1').
    const creds = await getDaemonWs()
    const resp = await fetch(`${daemonHttpBase(creds)}/boot-status`, {
      signal: AbortSignal.timeout(timeoutMs),
    })
    if (!resp.ok) {
      // 404 ⇒ pre-0.39.5 daemon (no /boot-status route) — OR the wedged
      // pool's tunnel-edge 404. Re-read the port file next poll in case a
      // kickstart moved it.
      invalidateDaemonWs()
      return { kind: 'http', httpStatus: resp.status }
    }
    return { kind: 'ok', status: (await resp.json()) as DaemonBootStatus }
  } catch {
    // Network error, timeout, port file missing, unparseable body — daemon
    // isn't reachable yet. Invalidate cached port so the next poll re-reads
    // ~/.k2so/daemon.port (covers a kickstart-assigned port change).
    invalidateDaemonWs()
    return { kind: 'network' }
  }
}

// ── Wedge detector (0.40.48 connection resilience) ─────────────────────────
//
// THE INCIDENT: after a remote server reboot, WKWebView kept reusing a
// poisoned pooled HTTP/2 connection whose every request failed at the tunnel
// edge with HTTP 404 ("no route found"). The connection is TRANSPORT-healthy
// (so the pool never evicts it, and JS has no eviction lever), but
// HTTP-level dead — the recovery poll itself rode the same pool, so
// recovery.kind sat on 'reconnecting' forever. Only a full app restart
// cleared it.
//
// THE DETECTOR: a sustained run of {kind:'http'} boot probes (transport
// works, HTTP fails) is the wedge signature — a genuine reboot produces
// 'network' errors, or resolves quickly. After WEDGE_PATTERN_MS of it we ask
// the Rust-side ARBITER (`remote_boot_probe` — a FRESH OS-level reqwest
// socket, completely outside the webview's pool) for a second opinion. If
// the arbiter reaches the daemon and sees phase 'ready' while the webview
// still can't, the pool is PROVEN poisoned → escalate:
//   step 1 — auto `window.location.reload()` once (guarded by a
//            sessionStorage flag; a reload tears down the page's fetch
//            context and usually gets a fresh pool);
//   step 2 — if the pattern re-establishes after the reload, surface the
//            'wedged' recovery state: banner copy + a Restart K2 button
//            (user click only — never an auto-restart).

/** How long the transport-healthy-but-HTTP-failing pattern must persist
 *  before the out-of-webview arbiter is consulted. */
export const WEDGE_PATTERN_MS = 60_000
/** Minimum spacing between arbiter probes while the pattern persists (the
 *  arbiter is cheap, but there's no point re-proving a wedge every tick). */
const WEDGE_ARBITER_MIN_INTERVAL_MS = 30_000
/** sessionStorage flag: the step-1 auto-reload already fired this page
 *  load for this host. Cleared on a healthy accept, so a NEW wedge months
 *  later can auto-reload again — but a reload that lands straight back in
 *  the wedge goes to step 2 instead of loop-reloading. */
function wedgeReloadFlagKey(hostKey: string): string {
  return `k2.wedge-reloaded:${hostKey}`
}

/** Pure rule: has the consecutive-http-failure run lasted long enough to
 *  consult the arbiter? `httpFailingSince` is the timestamp of the FIRST
 *  probe in the current uninterrupted {kind:'http'} run (null = no run). */
export function isWedgePatternEstablished(opts: {
  httpFailingSince: number | null
  now: number
}): boolean {
  return opts.httpFailingSince !== null && opts.now - opts.httpFailingSince >= WEDGE_PATTERN_MS
}

/** Pure rule: does the arbiter's out-of-webview /boot-status result prove
 *  the host healthy? TRUE iff it answered 2xx with a parseable body whose
 *  `phase` is 'ready' — combined with the webview's own probes still
 *  failing, that is the poisoned-pool proof. `null` = the arbiter couldn't
 *  reach the daemon either (genuine outage, NOT a wedge). */
export function arbiterProvesHostReady(
  probe: { status: number; body: string } | null,
): boolean {
  if (probe === null) return false
  if (probe.status < 200 || probe.status >= 300) return false
  try {
    const parsed = JSON.parse(probe.body) as { phase?: unknown }
    return parsed !== null && typeof parsed === 'object' && parsed.phase === 'ready'
  } catch {
    return false
  }
}

/** Run the Rust arbiter probe against a specific remote host. Returns the
 *  raw {status, body} or null when the probe failed at the network level
 *  (or we're outside a Tauri context). */
async function arbiterBootProbe(host: ConnectHost): Promise<{ status: number; body: string } | null> {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<{ status: number; body: string }>('remote_boot_probe', {
      hostname: host.hostname,
      port: host.port,
      secure: host.secure,
    })
  } catch (err) {
    // The fresh out-of-webview socket couldn't reach the daemon either —
    // that's a genuine outage signal, not a wedge. (Also covers non-Tauri
    // dev contexts where the command doesn't exist.)
    console.debug('[connection-gate] arbiter boot probe failed:', err)
    return null
  }
}

/**
 * 0.39.36 — stale connect-session reconnect fix.
 *
 * Connect-user login sessions are IN-MEMORY in the daemon
 * (connect_users.rs: a `OnceLock<Mutex<HashMap<token, Session>>>`), so a
 * daemon restart (host update / reboot / crash) WIPES them. `/boot-status`
 * is a PUBLIC route — it answers 200 'ready' regardless of session
 * validity — so the remote policy would happily 'accept' a host whose
 * token now points at a dead session. The app then mounts and every
 * authenticated `/cli/*` route 403s ("Invalid or missing auth token") →
 * a broken app (no file tree, no chat history, no terminal). The fix:
 * after the policy accepts a REMOTE host that HAS a token, probe the
 * session with `GET /cli/auth/whoami?token=…`; a 401/403 means the
 * session is dead → expire it (drop token) and surface RemoteSignIn for a
 * one-time re-auth instead of mounting a broken app.
 */

/** Outcome of the connect-session validity probe. */
type SessionProbe =
  | 'alive' // whoami 2xx — the session is valid; mount.
  | 'dead' // whoami 401/403 — the session is gone; re-auth.
  | 'unknown' // transport error / timeout — a network blip, NOT a dead
//             token; do NOT nuke the session. Proceed (mount) and let the
//             normal health-poll / a later real 403 sort it out.

/**
 * Map a whoami HTTP status (or null for a transport error/timeout) to a
 * session verdict. Pulled out as a pure fn so the "403 ⇒ dead vs blip ⇒
 * unknown" rule is unit-testable without the React/Tauri-bound effect.
 *
 *   - 401 / 403  → 'dead'    (stale/expired in-memory session — re-auth)
 *   - any 2xx    → 'alive'   (valid session — mount)
 *   - null       → 'unknown' (timeout/unreachable — a blip; do NOT expire)
 *   - other non-2xx (5xx/404/…) → 'unknown' (server hiccup, not an
 *     authoritative "your token is dead" — don't nuke a good session on it)
 */
export function classifyWhoamiStatus(httpStatus: number | null): SessionProbe {
  if (httpStatus === null) return 'unknown'
  if (httpStatus === 401 || httpStatus === 403) return 'dead'
  if (httpStatus >= 200 && httpStatus < 300) return 'alive'
  return 'unknown'
}

/**
 * Probe the ACTIVE remote host's connect-session via
 * `GET /cli/auth/whoami?token=…` (host-aware: getDaemonWs resolves the
 * active host's hostname/port/token). Returns:
 *   - 'dead'    on an authoritative 401/403 (session wiped by a restart),
 *   - 'alive'   on 2xx,
 *   - 'unknown' on a transport error/timeout or non-auth non-2xx (a blip).
 *
 * Time-bounded by REMOTE_WHOAMI_TIMEOUT_MS so a network hiccup can never
 * hang the gate. Caller only acts on 'dead'.
 */
async function probeRemoteSession(): Promise<SessionProbe> {
  try {
    const creds = await getDaemonWs()
    const resp = await fetch(
      `${daemonHttpBase(creds)}/cli/auth/whoami?token=${creds.token}`,
      { method: 'GET', signal: AbortSignal.timeout(REMOTE_WHOAMI_TIMEOUT_MS) },
    )
    return classifyWhoamiStatus(resp.status)
  } catch {
    // Network error / timeout / abort — a blip, not an authoritative
    // "dead token". Do NOT expire the session on this.
    return 'unknown'
  }
}

type AppComponent = React.ComponentType

// `activeHostKey` — the stable host identity that keys the <App> remount —
// now lives in the connect-host store so the per-machine session stores
// can reuse it without importing this React component (#625). Imported
// above.

export function ConnectionGate(): React.ReactElement {
  const [decision, setDecision] = useState<GateDecision>({ kind: 'wait', reason: 'starting' })
  const [attempts, setAttempts] = useState(0)
  const [AppModule, setAppModule] = useState<AppComponent | null>(null)
  // Fix C: the Phase-2 dynamic import of App exhausted its retries. On a
  // LOCAL accept the poll loop has stopped (attempts frozen), so without
  // this flag the overlay's Reload button could never appear — a failed
  // import was a dark "Connecting…" forever.
  const [importFailed, setImportFailed] = useState(false)
  // Cache the resolved app version so we don't re-invoke Tauri on every
  // host switch. The chosen POLICY, however, is rebuilt per host kind
  // (local vs remote) — see ensurePolicy.
  const appVersionRef = useRef<string | null | undefined>(undefined)
  // K2 Connect step #4 (soft reconnect): for a REMOTE host that has
  // already connected once this session, a transient drop should dim +
  // overlay the last view rather than blank the app. We track the
  // host-key that has reached 'accept' at least once. Local is never
  // soft-reconnected (the blank is correct for the auto-update race).
  const [connectedOnceKey, setConnectedOnceKey] = useState<string | null>(null)
  // The active remote's THREE-STATE recovery surface (owner contract —
  // lib/remote-recovery.ts): 'reconnecting' (down / still booting, debounced
  // for network blips), 'reauthenticating' (silent re-login underway), or
  // 'signin-required' (stored credentials rejected — the only state that
  // asks the user). Committed by this gate's poll + lib/remote-session's
  // revival; rendered as the non-blocking RecoveryBanner over the live app.
  const recovery = useConnectHostStore((s) => s.recovery)

  // K2 Connect step #1: the gate is host-aware. `hostKey` changes when
  // the user picks a different daemon in the top-bar switcher → the
  // polling effect below re-runs against the new host, and the <App>
  // element is keyed by it so a switch remounts the app cleanly (all
  // sockets re-open through the new host's creds).
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hostKey = activeHostKey(activeHost)
  const isRemote = activeHost !== 'local'
  // A remote host with NO session token must never be mounted: /boot-status
  // is a PUBLIC route (no token gate), so the policy would happily 'accept'
  // a tokenless remote — then App mounts and every host-aware / proxied
  // call fires with an empty token and the daemon answers "Invalid or
  // missing auth token". Treat a tokenless active remote as "needs
  // sign-in" so we surface RemoteSignIn instead of mounting.
  const remoteNeedsAuth =
    activeHost !== 'local' && (!activeHost.token || activeHost.token.length === 0)
  // K2 Connect step #3: a host the user picked that needs a password
  // (no remembered/expired token). Rendered as the full-screen sign-in.
  const pendingSignIn = useConnectHostStore((s) => s.pendingSignIn)

  // A tokenless active remote (e.g. an expired session restored from the
  // address book, or a boot where the keychain token didn't resolve)
  // drops into the full-screen sign-in rather than polling toward a mount
  // it could never authenticate. Idempotent: requestSignIn no-ops if the
  // same host is already pending.
  useEffect(() => {
    // `remoteNeedsAuth` already implies the active host is a remote with
    // no token; re-read it from the store so the narrowing is explicit.
    if (!remoteNeedsAuth) return
    const host = useConnectHostStore.getState().activeHost
    if (host !== 'local') {
      useConnectHostStore.getState().requestSignIn(host)
    }
    // activeHost identity (via hostKey) + the token presence drive this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hostKey, remoteNeedsAuth])

  // K2 Connect step #3: hydrate the saved address book + resolve
  // remembered tokens from the keychain ONCE at boot, before any remote
  // auto-sign-in can fire. Local boot is unaffected (default activeHost
  // is 'local', and hydration only fills the hosts[] list + tokens).
  useEffect(() => {
    void useConnectHostStore.getState().hydrateFromDisk()
  }, [])

  // Phase 1: resolve the app version once, then poll the ACTIVE host's
  // /boot-status until the acceptance policy says to mount. Re-runs when
  // the active host changes (hostKey dep).
  useEffect(() => {
    let cancelled = false
    let timeoutId: ReturnType<typeof setTimeout> | null = null
    // Closure-local poll counter. `attempts` state is frozen at effect
    // setup (deps = [hostKey]); this tracks attempts since the last
    // accept for the soft-reconnect backoff curve.
    let attemptsLocal = 0
    // Has this host reached 'accept' at least once during THIS effect
    // run? Gates the post-accept remote health-poll + soft backoff.
    let acceptedOnce = false
    // Consecutive FAILED health-polls since the last accept (closure-local
    // so it survives across ticks but resets per host switch). Debounces a
    // remote drop: a single blip is swallowed; only >= REMOTE_DROP_THRESHOLD
    // in a row counts as a genuine reconnect. Reset to 0 on every accept.
    let consecutiveFails = 0
    // Fix B (upgrade-kickstart stale creds): has THIS effect run seen any
    // non-accept poll? If a LOCAL accept follows one, we crossed a daemon
    // restart while polling and the cached port/token are stale — see
    // shouldRefreshCredsOnAccept. Covers the first-poll-errors case too
    // (fetchBootStatus null → wait → flag set → later accept refreshes).
    let sawNonAccept = false
    // Wedge detector (0.40.48): start of the current uninterrupted run of
    // {kind:'http'} boot probes (transport healthy, HTTP failing — the
    // poisoned-pool signature). Reset by any 'ok' or 'network' probe.
    let httpFailingSince: number | null = null
    // Throttle for the out-of-webview arbiter probe.
    let lastArbiterAt = 0
    // Step 2 latched: the wedge survived the auto-reload; the 'wedged'
    // recovery state is up and must not be clobbered by the ordinary
    // reconnecting-banner writes below.
    let wedgeConfirmed = false
    // instanceId plumbing (0.40.48): the daemon's per-boot id seen at the
    // first accept of THIS effect run. A different id on a later accepted
    // poll proves the server restarted between health ticks.
    let acceptedInstanceId: string | null = null

    // A host switch must re-poll from scratch: drop any prior accept so
    // the overlay shows while the new host is contacted. (The recovery
    // banner state resets in selectHost, which owns the switch.)
    setDecision({ kind: 'wait', reason: 'switching-host' })
    setAttempts(0)

    const ensurePolicy = async (): Promise<AcceptancePolicy> => {
      // Select the policy by active-host KIND (K2 Connect step #4):
      //   - 'local' → localPairedPolicy: exact version match (the
      //     auto-update pairing guard — never mount the outgoing old
      //     daemon mid-update).
      //   - a ConnectHost → remoteHostPolicy: protocol range-check +
      //     phase ready; a remote may run a different marketing version,
      //     so version-string equality must NOT be required.
      // Rebuilt per effect-run (host switch), so the right policy is
      // always paired with the current host.
      if (isRemote) return remoteHostPolicy()
      if (appVersionRef.current === undefined) {
        appVersionRef.current = await getAppVersion()
      }
      return localPairedPolicy(appVersionRef.current)
    }

    const tick = async (): Promise<void> => {
      // Never poll toward a mount we can't authenticate: a remote host
      // with no session token must sign in first (the effect above opens
      // RemoteSignIn). Park in 'wait' until a token lands — selectHost /
      // setHostToken re-key this effect when the session is obtained.
      if (remoteNeedsAuth) {
        if (cancelled) return
        setDecision({ kind: 'wait', reason: 'remote-needs-auth' })
        useConnectHostStore.getState().setConnectionStatus('connecting')
        return
      }
      const policy = await ensurePolicy()
      // Remote health-polls get a looser timeout (a tunnel round-trip is
      // higher-latency than a localhost hit); local keeps the tight 2s.
      const probe = await fetchBootStatus(
        isRemote ? REMOTE_BOOT_STATUS_TIMEOUT_MS : 2000,
      )
      if (cancelled) return
      // Policies keep their old input shape: any failed probe folds to null.
      const status = probe.kind === 'ok' ? probe.status : null
      // Wedge tracking (0.40.48): only a REMOTE host can wedge (the local
      // loopback daemon has no tunnel edge / shared pooled origin). Track
      // consecutive transport-healthy-but-HTTP-failing probes; any 'ok' or
      // 'network' outcome breaks the run.
      if (isRemote && probe.kind === 'http') {
        if (httpFailingSince === null) httpFailingSince = Date.now()
      } else {
        httpFailingSince = null
      }
      if (
        isRemote &&
        !wedgeConfirmed &&
        isWedgePatternEstablished({ httpFailingSince, now: Date.now() }) &&
        Date.now() - lastArbiterAt >= WEDGE_ARBITER_MIN_INTERVAL_MS
      ) {
        lastArbiterAt = Date.now()
        const active = useConnectHostStore.getState().activeHost
        if (active !== 'local') {
          const verdict = await arbiterBootProbe(active)
          if (cancelled) return
          if (arbiterProvesHostReady(verdict)) {
            // PROVEN: a fresh OS-level socket reaches the daemon and it's
            // 'ready', while the webview's own probes have failed at the
            // HTTP layer for ≥ WEDGE_PATTERN_MS. The webview pool is
            // poisoned. Escalate.
            const flagKey = wedgeReloadFlagKey(hostKey)
            if (sessionStorage.getItem(flagKey) !== '1') {
              // Step 1 (once per page load): a reload rebuilds the page's
              // fetch context, which usually gets a fresh connection pool.
              console.warn(
                '[connection-gate] webview connection pool is wedged for',
                active.hostname,
                '(arbiter reached the daemon; webview cannot) — auto-reloading to clear it',
              )
              sessionStorage.setItem(flagKey, '1')
              window.location.reload()
              return
            }
            // Step 2: the reload didn't clear it — only an app restart can.
            // Latch the 'wedged' state; the RecoveryBanner renders the copy
            // + the Restart K2 button. NEVER auto-restart.
            console.error(
              '[connection-gate] wedge persisted through a page reload for',
              active.hostname,
              '— surfacing Restart K2',
            )
            wedgeConfirmed = true
            useConnectHostStore.getState().setRecovery({ kind: 'wedged' })
          }
        }
      }
      let next = policy.decide(status)
      // 0.39.36: a REMOTE host's /boot-status accepting only proves the
      // daemon is up + the right protocol — NOT that this client's
      // in-memory connect-session survived the daemon's last restart. On
      // the FIRST accept of a token-bearing remote, validate the session
      // with a cheap host-aware whoami probe BEFORE mounting. A dead
      // (401/403) session → expire the token + drop to RemoteSignIn rather
      // than mounting an app where every /cli/* call 403s. A transport
      // blip ('unknown') is NOT treated as dead — we proceed to mount. We
      // probe only on the first accept (acceptedOnce false): once mounted,
      // the ongoing health-poll + real /cli/* 401 handlers own expiry, so
      // a transient 403 mid-session never nukes a working app here.
      if (
        next.kind === 'accept' &&
        isRemote &&
        !acceptedOnce &&
        !remoteNeedsAuth
      ) {
        const active = useConnectHostStore.getState().activeHost
        const hasToken =
          active !== 'local' && !!active.token && active.token.length > 0
        if (hasToken) {
          // Retry on 'unknown' so a real network blip resolves to 'alive'; a
          // token confirmed dead ('dead') short-circuits the loop immediately.
          let probe = await probeRemoteSession()
          for (
            let i = 0;
            probe === 'unknown' && i < FIRST_CONNECT_PROBE_RETRIES;
            i++
          ) {
            await new Promise((r) =>
              setTimeout(r, FIRST_CONNECT_PROBE_RETRY_DELAY_MS),
            )
            if (cancelled) return
            probe = await probeRemoteSession()
          }
          if (cancelled) return
          if (probe !== 'alive') {
            // 'dead' (401/403) OR persistent 'unknown' (the session could not
            // be confirmed after retries — a dead remembered token wiped by a
            // remote update/restart, or an auth probe that never completes
            // through the E2E splice). Issue #4: NEVER strand the client by
            // mounting an unverified token — it sits on "Connecting…" forever
            // and never reaches sign-in. Drop the token + raise RemoteSignIn
            // for a one-time re-auth. This stricter rule is FIRST-CONNECT only
            // (acceptedOnce false); a mounted session still tolerates a blip.
            // `hasToken` already narrowed `active` to a ConnectHost.
            useConnectHostStore.getState().expireSession(active.id)
            next = { kind: 'wait', reason: 'remote-session-expired' }
          } else {
            // 'alive' → keep the accept and mount as normal; the recovery
            // surface starts (or returns to) the healthy baseline.
            useConnectHostStore.getState().setRecovery({ kind: 'connected' })
          }
        }
      }
      // Runtime staleness probe (mirrors the daemon's own 5s WS re-auth
      // heartbeat, from the other side): /boot-status is PUBLIC, so a remote
      // restart/update that WIPED the in-memory connect-sessions still polls
      // 'accept' here while every authed /cli/* call 403s and every WS
      // reconnect loop spins on the dead token. Probe the session on each
      // post-accept health tick; a confirmed-dead (401/403) session goes
      // through the single-flight reviveRemoteSession — the SAME re-login
      // flow boot uses — so the app self-heals in place instead of needing a
      // relaunch. Only 'dead' acts ('unknown' is a blip); revival itself
      // expires the token + raises RemoteSignIn only when the remembered
      // password is missing/rejected, and its backoff caps re-login attempts.
      //
      // Recovery contract: 'dead' here is state 2 ('reauthenticating') —
      // reviveRemoteSession paints it and folds its outcome back through the
      // reducer (connected / signin-required / still reauthenticating).
      // Awaited so this tick's poll cadence sees the settled state.
      if (next.kind === 'accept' && isRemote && acceptedOnce) {
        const active = useConnectHostStore.getState().activeHost
        if (active !== 'local' && !!active.token && active.token.length > 0) {
          const probe = await probeRemoteSession()
          if (cancelled) return
          if (probe === 'dead') {
            await reviveRemoteSession(active.id)
            if (cancelled) return
          } else {
            // 'alive', or a blip ('unknown' is never evidence of staleness):
            // the host is up+ready and the session holds → connected. This
            // also clears a prior 'reconnecting' banner on recovery.
            useConnectHostStore.getState().setRecovery(
              deriveRecovery({
                bootStatus: { reachable: true, phase: 'ready' },
                auth: probe === 'alive' ? 'ok' : 'unknown',
              }),
            )
          }
        }
      }
      // Debounced-drop path: a REMOTE host that has already connected this
      // effect-run (post-accept health-poll) must not surface a single
      // blipped poll. Below REMOTE_DROP_THRESHOLD consecutive fails we keep
      // the gate 'connected' with no banner — the data WS is likely still
      // streaming — and only the accept branch / threshold branch below
      // update `decision` + status. So skip the unconditional setters here
      // for that case and let the branches own them.
      const softPoll = isRemote && acceptedOnce
      if (!softPoll) {
        setDecision(next)
        // Surface the active host's live status to the top-bar switcher.
        useConnectHostStore.getState().setConnectionStatus(
          next.kind === 'accept' ? 'connected' : 'connecting',
        )
      }
      if (next.kind === 'accept') {
        // instanceId plumbing (0.40.48): the daemon stamps /boot-status with
        // a per-boot id. If an accepted poll carries a DIFFERENT id than the
        // one we accepted earlier this effect run, the server restarted
        // between health ticks (possibly without a single failed poll —
        // fast reboots + the 4s cadence can hide the gap entirely). Drop the
        // cached creds and let this same accept flow re-run everything: the
        // whoami probe / revival path owns the (now wiped) session, and the
        // WS factories reconnect + re-snapshot via their hello handlers.
        // Older daemons omit instanceId — every branch below tolerates that.
        if (status?.instanceId) {
          if (acceptedInstanceId === null) {
            acceptedInstanceId = status.instanceId
          } else if (acceptedInstanceId !== status.instanceId) {
            console.warn(
              `[connection-gate] daemon instanceId changed (${acceptedInstanceId} → ${status.instanceId}) — server restarted; refreshing creds`,
            )
            invalidateDaemonWs()
            acceptedInstanceId = status.instanceId
          }
        }
        // A healthy accept ends any wedge episode: clear the tracker, the
        // step-2 latch, and the step-1 reload flag (so a NEW wedge later in
        // this app run gets its auto-reload chance again).
        httpFailingSince = null
        wedgeConfirmed = false
        sessionStorage.removeItem(wedgeReloadFlagKey(hostKey))
        // Fix B: a LOCAL accept after any non-accept poll means the daemon
        // restarted under us (upgrade kickstart) — it rotated its boot
        // token and rebound the same port, and /boot-status is public, so
        // this accept says nothing about our CACHED creds. Drop them so
        // App mounts against a fresh port+token read instead of a 401
        // storm. Synchronous, before any state flush can mount App.
        if (shouldRefreshCredsOnAccept({ isRemote, sawNonAccept })) {
          invalidateDaemonWs()
        }
        // A soft health-poll skipped the unconditional setters above; apply
        // them on recovery so the gate flips back to 'connected' and the
        // banner clears.
        if (softPoll) {
          setDecision(next)
          useConnectHostStore.getState().setConnectionStatus('connected')
        }
        // #638: cache the accepted host's version + protocol so
        // lib/server-capabilities can gate newer client features against an
        // older host (and build "update the host to vX" hints). For a
        // REMOTE host we use the host's own /boot-status `version`; for
        // LOCAL we use this app's bundled version (the daemon is paired with
        // the app, but caching the real version keeps gating uniform).
        useConnectHostStore.getState().setServerInfo({
          version: status?.version ?? (isRemote ? null : appVersionRef.current ?? null),
          protocol: status?.protocol ?? null,
        })
        // Remember this host reached 'accept' at least once → a later
        // drop becomes a SOFT reconnect (overlay) instead of a blank.
        acceptedOnce = true
        setConnectedOnceKey(hostKey)
        attemptsLocal = 0
        // A clean poll clears the debounce: any in-flight blip count is
        // forgotten and the reconnect banner (if shown) comes down.
        consecutiveFails = 0
        if (isRemote) {
          // Remote: keep a slow health-poll alive so a tunnel drop is
          // detected and surfaced as the soft-reconnect overlay (the App
          // stays mounted). Local stops polling on accept — its only
          // re-entry is an intentional auto-update/host-switch remount.
          timeoutId = setTimeout(() => { void tick() }, REMOTE_HEALTH_POLL_MS)
          return
        }
        return // local: stop polling; Phase 2 takes over
      }
      // Non-accept (a failed poll). Remember it: a later accept now implies
      // we crossed a daemon restart → cached creds must be refreshed (fix B).
      sawNonAccept = true
      // For a soft health-poll we DEBOUNCE: a
      // single blip is swallowed (gate stays 'connected', no banner) and
      // only >= REMOTE_DROP_THRESHOLD consecutive fails surface the wait
      // decision + the non-blocking reconnect banner. The App stays mounted
      // throughout; recovery (an accept above) clears the counter + banner.
      if (softPoll) {
        consecutiveFails += 1
        // Recovery state 1 — the "still booting back up" affordance. A host
        // that ANSWERED /boot-status with a pre-'ready' phase (e.g.
        // 'migrating' while an update applies) is AUTHORITATIVELY restarting:
        // surface immediately, carrying the phase so the banner can say why.
        // A network-level failure (status null) is debounced behind
        // REMOTE_DROP_THRESHOLD so a single tunnel blip never flashes the
        // banner while the data WS is still streaming. Never a login prompt
        // in this state — auth is unknowable until the host is back.
        const bootingAuthoritative = status !== null && status.phase !== 'ready'
        if (bootingAuthoritative || shouldSurfaceRemoteDrop(consecutiveFails)) {
          setDecision(next)
          useConnectHostStore.getState().setConnectionStatus('connecting')
          // A latched 'wedged' state outranks the ordinary reconnecting
          // banner — the poll keeps running (a surprise recovery still
          // clears it via the accept branch), but it must not repaint the
          // banner back to "reconnecting automatically" when it can't.
          if (!wedgeConfirmed) {
            useConnectHostStore.getState().setRecovery(
              deriveRecovery({
                bootStatus: bootingAuthoritative
                  ? { reachable: true, phase: status.phase }
                  // A reachable-but-rejected edge (e.g. incompatible protocol
                  // after an update) renders as generic reconnecting, never as
                  // 'connected' — fold it into the unreachable input.
                  : { reachable: false },
                auth: 'unknown',
              }),
            )
          }
        }
        // else: below threshold — leave 'connected' + no banner untouched.
      }
      setAttempts((a) => a + 1)
      attemptsLocal += 1
      // Backoff (recoveryPollMs is the tested schedule):
      //   - First connect (local, or a remote not yet accepted): tight 500ms.
      //   - Soft poll still WITHIN the debounce window (recovery still
      //     'connected' — no banner yet): normal health cadence so we detect
      //     recovery / cross the threshold promptly.
      //   - Confirmed 'reconnecting' (banner up): ease off exponentially so
      //     we don't hammer a down/booting host.
      // 0.40.48: jittered at the call site (recoveryPollMs stays pure /
      // deterministic for its unit tests) so many clients rebooting off the
      // same host don't re-align into a synchronized poll storm.
      const backoff = jittered(
        softPoll
          ? recoveryPollMs(useConnectHostStore.getState().recovery, attemptsLocal)
          : 500,
      )
      timeoutId = setTimeout(() => { void tick() }, backoff)
    }

    void tick()

    return () => {
      cancelled = true
      if (timeoutId !== null) clearTimeout(timeoutId)
    }
    // isRemote is fully determined by hostKey (local key === 'local').
    // `remoteNeedsAuth` is added so that OBTAINING a session token (same
    // host, so hostKey is unchanged) re-runs the effect: the prior run
    // parked in 'wait' and scheduled no further tick, so without this dep
    // the gate would never resume polling toward 'accept'.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hostKey, remoteNeedsAuth])

  // Phase 2: once accepted, dynamically import App. Its import
  // side-effects (store creation, eager fetches) run NOW for the first
  // time, against a daemon confirmed to be the right version AND ready.
  //
  // Fix C: the import is retried with backoff — a chunk request can die
  // when the webview was natively recovered mid-boot (the black-screen
  // watchdog path) or the asset protocol hiccups. On exhausted retries we
  // set importFailed so the overlay says WHY and surfaces Reload, instead
  // of console.error-ing into an eternal "Connecting…".
  useEffect(() => {
    if (decision.kind !== 'accept') return
    let cancelled = false
    let retryId: ReturnType<typeof setTimeout> | null = null
    const attemptImport = (attempt: number): void => {
      void import('../App').then((mod) => {
        if (cancelled) return
        setImportFailed(false)
        setAppModule(() => mod.default)
      }).catch((err: unknown) => {
        console.error(
          `[ConnectionGate] dynamic import of App failed (attempt ${attempt + 1}/${APP_IMPORT_RETRY_DELAYS_MS.length + 1}):`,
          err,
        )
        if (cancelled) return
        const delay = APP_IMPORT_RETRY_DELAYS_MS[attempt]
        if (delay !== undefined) {
          retryId = setTimeout(() => { attemptImport(attempt + 1) }, delay)
        } else {
          // Out of retries — stop pretending we're connecting.
          setImportFailed(true)
        }
      })
    }
    attemptImport(0)
    return () => {
      cancelled = true
      if (retryId !== null) clearTimeout(retryId)
    }
  }, [decision.kind])

  // K2 Connect step #4 — soft reconnect. A REMOTE host that has already
  // connected this session (App mounted) must stay MOUNTED + INTERACTIVE
  // through transient drops — never fall back to the full-screen
  // ConnectingOverlay/blank. We key this purely on "has connected once +
  // App is mounted" (NOT the current decision): a sub-threshold blip leaves
  // the App fully usable with NO banner, and only a non-'connected'
  // `recovery` state (debounced drop / authoritative boot phase / re-auth /
  // sign-in-required) adds the small non-blocking banner over the still-live
  // view. Local — and a remote's FIRST connect — keep the full-screen
  // blanking overlay (correct for the auto-update race /
  // nothing-to-show-yet).
  const keepRemoteMounted =
    activeHost !== 'local' && connectedOnceKey === hostKey && AppModule !== null

  // K2 Connect step #3 — full-screen sign-in for a picked host with no
  // remembered/valid token. Rendered ON TOP of the current view (the
  // last-mounted App if there is one, else the connecting overlay) so the
  // user's place is preserved while they re-auth a single server.
  const signInOverlay = pendingSignIn ? <RemoteSignIn host={pendingSignIn} /> : null

  if (keepRemoteMounted) {
    const App = AppModule
    return (
      <>
        <AppErrorBoundary><App key={hostKey} /></AppErrorBoundary>
        {recovery.kind !== 'connected' && (
          <RecoveryBanner host={activeHost} recovery={recovery} />
        )}
        {signInOverlay}
      </>
    )
  }

  if (decision.kind !== 'accept' || AppModule === null) {
    return (
      <>
        <ConnectingOverlay decision={decision} attempts={attempts} importFailed={importFailed} />
        {/* 0.40.48: a wedge that survived the step-1 auto-reload latches
            BEFORE the app can mount (the poisoned pool blocks the accept),
            so the wedged banner must also render over the connecting
            overlay — otherwise the user is back to an unexplained eternal
            "Connecting…", the exact incident this fixes. */}
        {recovery.kind === 'wedged' && activeHost !== 'local' && (
          <RecoveryBanner host={activeHost} recovery={recovery} />
        )}
        {signInOverlay}
      </>
    )
  }

  const App = AppModule
  // Key by the active host so switching daemons unmounts + remounts App
  // wholesale — every store, WS, and terminal pane re-initializes against
  // the new host's creds rather than clinging to the old socket.
  return (
    <>
      <AppErrorBoundary><App key={hostKey} /></AppErrorBoundary>
      {signInOverlay}
    </>
  )
}

/** Small, NON-BLOCKING recovery indicator shown over the still-live view
 *  while a previously-connected REMOTE host recovers (K2 Connect step #4 +
 *  the three-state owner contract). Unlike the old full-screen overlay:
 *    - it does NOT cover the app or the top-bar (a bottom-center pill),
 *    - it passes ALL input through (`pointerEvents: 'none'`) so the app —
 *      and the top-bar host switcher — stay fully usable while we retry.
 *      The one interactive element is the 'signin-required' Sign-in button
 *      (pointerEvents re-enabled on the pill for that state only), which
 *      routes straight to RemoteSignIn — never a dead end.
 *  'reconnecting' only mounts after the drop is debounced (or the host
 *  authoritatively reports a pre-ready boot phase), so a single blip over a
 *  higher-latency tunnel never flashes it while the data WS is streaming. */
function RecoveryBanner({
  host,
  recovery,
}: {
  host: ConnectHost
  recovery: RemoteRecoveryState
}): React.ReactElement {
  // Both states that need the user re-enable pointer events on the pill:
  // 'signin-required' (Sign in button) and 'wedged' (Restart K2 button —
  // the webview's connection pool is proven poisoned; only an app restart
  // clears it, and it is ALWAYS user-initiated, never automatic).
  const needsUser = recovery.kind === 'signin-required' || recovery.kind === 'wedged'
  return (
    <div
      role="status"
      aria-live="polite"
      style={{
        // Non-blocking: a pill pinned to the bottom-center, well clear of
        // the top-bar, that lets every click/keystroke fall through to the
        // app underneath.
        position: 'fixed',
        left: 0,
        right: 0,
        bottom: '1.25rem',
        zIndex: 9999,
        display: 'flex',
        justifyContent: 'center',
        pointerEvents: 'none',
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: '0.5rem',
          padding: '0.4rem 0.85rem',
          borderRadius: '999px',
          background: 'rgba(20,20,20,0.92)',
          border: '1px solid var(--color-border, rgba(255,255,255,0.12))',
          boxShadow: '0 2px 12px rgba(0,0,0,0.35)',
          color: 'var(--color-text-primary, #e0e0e0)',
          fontFamily: 'system-ui, -apple-system, sans-serif',
          fontSize: '0.8rem',
          userSelect: 'none',
          WebkitUserSelect: 'none',
          // Only the sign-in state accepts input (its button below).
          pointerEvents: needsUser ? 'auto' : 'none',
        }}
      >
        {/* Status dot: amber = degraded-but-automatic; red = needs the user. */}
        <span
          aria-hidden
          style={{
            width: '8px',
            height: '8px',
            borderRadius: '50%',
            background: needsUser ? '#f85149' : '#f5a623',
            flexShrink: 0,
          }}
        />
        <span>{recoveryStatusText(host.label, recovery)}</span>
        {recovery.kind === 'wedged' && (
          <button
            onClick={() => {
              // User-initiated ONLY. restart_app rides the proven
              // helper-script relaunch (settings.rs) so the .app bundle
              // reopens cleanly with a fresh WebKit networking process —
              // the one thing that evicts the poisoned pool.
              void import('@tauri-apps/api/core').then(({ invoke }) => invoke('restart_app'))
            }}
            style={{
              padding: '0.15rem 0.6rem',
              fontSize: '0.75rem',
              borderRadius: '999px',
              border: '1px solid var(--color-border, rgba(255,255,255,0.25))',
              background: 'var(--color-accent, #2f6feb)',
              color: 'var(--color-on-accent)',
              cursor: 'pointer',
            }}
          >
            Restart K2
          </button>
        )}
        {recovery.kind === 'signin-required' && (
          <button
            onClick={() => {
              // Route straight to the login surface for THIS host — the one
              // state that legitimately needs the user.
              useConnectHostStore.getState().requestSignIn(host)
            }}
            style={{
              padding: '0.15rem 0.6rem',
              fontSize: '0.75rem',
              borderRadius: '999px',
              border: '1px solid var(--color-border, rgba(255,255,255,0.25))',
              background: 'var(--color-accent, #2f6feb)',
              color: 'var(--color-on-accent)',
              cursor: 'pointer',
            }}
          >
            Sign in
          </button>
        )}
      </div>
    </div>
  )
}

interface ConnectingOverlayProps {
  decision: GateDecision
  attempts: number
  /** Fix C: the Phase-2 App import exhausted its retries — say so and
   *  always surface the Reload button (attempts may be frozen). */
  importFailed: boolean
}

function ConnectingOverlay({ decision, attempts, importFailed }: ConnectingOverlayProps): React.ReactElement {
  const migrating = decision.kind === 'migrating'

  // Heading + sub-line. While the (correct) daemon is migrating we tell
  // the user updates are being applied; otherwise it's a plain connect.
  // A failed App import trumps both — "Connecting…" would be a lie (the
  // daemon accepted; it's the UI bundle that couldn't load).
  const heading = importFailed
    ? 'K2 failed to load'
    : migrating ? 'Setting up K2…' : 'Connecting…'
  const subline = importFailed
    ? 'The app interface could not be loaded.'
    : migrating
      ? (decision.detail && decision.detail.length > 0 ? decision.detail : 'Applying updates…')
      : null

  // Reload escape hatch. A big upgrade's migration sweep can legitimately
  // take a while, so don't nag during 'migrating' (only after ~60s). For
  // a plain 'wait' (unreachable / wrong version) surface it after ~10s.
  // importFailed shows it UNCONDITIONALLY: on a local accept the poll
  // loop stopped, so the attempt thresholds can never fire on their own.
  const showReloadButton = shouldShowReloadButton({ migrating, attempts, importFailed })

  return (
    <div
      style={{
        width: '100vw',
        height: '100vh',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        flexDirection: 'column',
        gap: '1.25rem',
        background: 'var(--color-bg, #0a0a0a)',
        color: 'var(--color-text-primary, #e0e0e0)',
        fontFamily: 'system-ui, -apple-system, sans-serif',
        userSelect: 'none',
        WebkitUserSelect: 'none',
        cursor: 'default',
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '0.5rem' }}>
        <div style={{ fontSize: '1rem', fontWeight: 500 }}>{heading}</div>
        {subline !== null && (
          <div style={{ fontSize: '0.85rem', opacity: 0.7 }}>{subline}</div>
        )}
      </div>
      {showReloadButton && (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '0.75rem' }}>
          <div style={{ fontSize: '0.85rem', opacity: 0.75, maxWidth: '440px', textAlign: 'center', lineHeight: 1.5 }}>
            {importFailed
              ? 'The app failed to load after several attempts. Reloading usually fixes this — if it keeps happening, quit and relaunch K2.'
              : migrating
                ? 'K2 is still applying updates. This can take a minute on a large upgrade — you can keep waiting, or reload below.'
                : "Your K2 daemon may still be loading. If you're unsure, quit and relaunch the app, or try reloading with the button below."}
          </div>
          <button
            onClick={() => { window.location.reload() }}
            style={{
              padding: '0.5rem 1.25rem',
              fontSize: '0.85rem',
              borderRadius: '4px',
              border: '1px solid var(--color-border, rgba(255,255,255,0.15))',
              background: 'var(--color-bg-elevated, rgba(255,255,255,0.05))',
              color: 'inherit',
              cursor: 'pointer',
            }}
          >
            Reload
          </button>
        </div>
      )}
    </div>
  )
}

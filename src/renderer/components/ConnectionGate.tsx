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
import { useConnectHostStore, activeHostKey } from '@/stores/connect-host'
import { RemoteSignIn } from './RemoteSignIn'

/** Shape of the daemon's GET /boot-status response. `detail` is free-text
 *  for the UI only — never branch on it. */
interface DaemonBootStatus {
  version: string
  protocol: number
  phase: string // 'starting' | 'migrating' | 'ready' | 'error' | future
  detail: string
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
/** Upper bound for the exponential reconnect backoff once a connected
 *  remote drops — avoids hammering a flaky tunnel. */
const REMOTE_RECONNECT_MAX_MS = 8000
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

/** Debounce rule for a connected-remote drop: given the running count of
 *  CONSECUTIVE failed health-polls, should we surface the reconnect banner
 *  yet? Pulled out as a pure fn so the threshold behaviour (N-1 fails →
 *  no banner; Nth → banner) is unit-testable without the React/Tauri-bound
 *  effect. Used by the poll loop below. */
export function shouldSurfaceRemoteDrop(consecutiveFails: number): boolean {
  return consecutiveFails >= REMOTE_DROP_THRESHOLD
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
        return { kind: 'wait', reason: `version ${status.version} != app ${expectedVersion}` }
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

/** Hit the daemon's /boot-status with a per-attempt timeout. Returns the
 *  parsed status, or null on any error / non-2xx (covers a pre-0.39.5
 *  daemon's 404, network error, timeout, missing port file). `timeoutMs`
 *  is looser for a remote health-poll than for a local boot (a tunnel
 *  round-trip is higher-latency — see REMOTE_BOOT_STATUS_TIMEOUT_MS). */
async function fetchBootStatus(timeoutMs = 2000): Promise<DaemonBootStatus | null> {
  try {
    // Host-aware (K2 Connect step #1): polls the ACTIVE host's
    // /boot-status. For 'local' this is byte-identical to before
    // (host === '127.0.0.1').
    const creds = await getDaemonWs()
    const resp = await fetch(`${daemonHttpBase(creds)}/boot-status`, {
      signal: AbortSignal.timeout(timeoutMs),
    })
    if (!resp.ok) {
      // 404 ⇒ pre-0.39.5 daemon (no /boot-status route). Re-read the port
      // file next poll in case a kickstart moved it.
      invalidateDaemonWs()
      return null
    }
    return (await resp.json()) as DaemonBootStatus
  } catch {
    // Network error, timeout, port file missing, etc. — daemon isn't
    // reachable yet. Invalidate cached port so the next poll re-reads
    // ~/.k2so/daemon.port (covers a kickstart-assigned port change).
    invalidateDaemonWs()
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
  // K2 Connect step #4 (debounced drop): a single slow/blipped health-poll
  // over a higher-latency tunnel must NOT surface anything while the data
  // WS is still streaming. We only flip this true after
  // REMOTE_DROP_THRESHOLD CONSECUTIVE failed polls; the gate then renders
  // the small non-blocking "reconnecting" banner. Reset to false on the
  // next accepting poll. Distinct from `decision` so a sub-threshold blip
  // leaves connectionStatus 'connected' and the App fully interactive.
  const [reconnecting, setReconnecting] = useState(false)

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

    // A host switch must re-poll from scratch: drop any prior accept so
    // the overlay shows while the new host is contacted.
    setDecision({ kind: 'wait', reason: 'switching-host' })
    setAttempts(0)
    setReconnecting(false)

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
      const status = await fetchBootStatus(
        isRemote ? REMOTE_BOOT_STATUS_TIMEOUT_MS : 2000,
      )
      if (cancelled) return
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
          const probe = await probeRemoteSession()
          if (cancelled) return
          if (probe === 'dead') {
            // Session was wiped (host restart/update/crash). Drop the dead
            // token + raise RemoteSignIn for a one-time re-auth. Re-key as
            // a wait so we don't fall through to the mount path below; the
            // effect re-runs on the resulting token change. `hasToken`
            // already narrowed `active` to a ConnectHost.
            useConnectHostStore.getState().expireSession(active.id)
            next = { kind: 'wait', reason: 'remote-session-expired' }
          }
          // 'alive' / 'unknown' → keep the accept and mount as normal.
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
        setReconnecting(false)
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
      // Non-accept (a failed poll). For a soft health-poll we DEBOUNCE: a
      // single blip is swallowed (gate stays 'connected', no banner) and
      // only >= REMOTE_DROP_THRESHOLD consecutive fails surface the wait
      // decision + the non-blocking reconnect banner. The App stays mounted
      // throughout; recovery (an accept above) clears the counter + banner.
      if (softPoll) {
        consecutiveFails += 1
        if (shouldSurfaceRemoteDrop(consecutiveFails)) {
          setDecision(next)
          useConnectHostStore.getState().setConnectionStatus('connecting')
          setReconnecting(true)
        }
        // else: below threshold — leave 'connected' + no banner untouched.
      }
      setAttempts((a) => a + 1)
      attemptsLocal += 1
      // Backoff:
      //   - First connect (local, or a remote not yet accepted): tight 500ms.
      //   - Soft poll still WITHIN the debounce window (no banner yet): keep
      //     the normal health cadence so we detect recovery / cross the
      //     threshold promptly rather than stretching out a swallowed blip.
      //   - Confirmed soft reconnect (banner up): ease off exponentially so
      //     we don't hammer a flaky tunnel.
      const backoff = softPoll
        ? shouldSurfaceRemoteDrop(consecutiveFails)
          ? Math.min(REMOTE_RECONNECT_MAX_MS, 500 * 2 ** Math.min(attemptsLocal, 5))
          : REMOTE_HEALTH_POLL_MS
        : 500
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
  useEffect(() => {
    if (decision.kind !== 'accept') return
    let cancelled = false
    void import('../App').then((mod) => {
      if (cancelled) return
      setAppModule(() => mod.default)
    }).catch((err: unknown) => {
      console.error('[ConnectionGate] dynamic import of App failed:', err)
    })
    return () => { cancelled = true }
  }, [decision.kind])

  // K2 Connect step #4 — soft reconnect. A REMOTE host that has already
  // connected this session (App mounted) must stay MOUNTED + INTERACTIVE
  // through transient drops — never fall back to the full-screen
  // ConnectingOverlay/blank. We key this purely on "has connected once +
  // App is mounted" (NOT the current decision): a sub-threshold blip leaves
  // the App fully usable with NO banner, and only the debounced
  // `reconnecting` flag (>= REMOTE_DROP_THRESHOLD consecutive fails) adds
  // the small non-blocking banner over the still-live view. Local — and a
  // remote's FIRST connect — keep the full-screen blanking overlay (correct
  // for the auto-update race / nothing-to-show-yet).
  const keepRemoteMounted =
    activeHost !== 'local' && connectedOnceKey === hostKey && AppModule !== null

  // K2 Connect step #3 — full-screen sign-in for a picked host with no
  // remembered/valid token. Rendered ON TOP of the current view (the
  // last-mounted App if there is one, else the connecting overlay) so the
  // user's place is preserved while they re-auth a single server.
  const signInOverlay = pendingSignIn ? <RemoteSignIn host={pendingSignIn} /> : null

  if (keepRemoteMounted) {
    const App = AppModule
    const label = activeHost.label
    return (
      <>
        <App key={hostKey} />
        {reconnecting && <ReconnectBanner label={label} />}
        {signInOverlay}
      </>
    )
  }

  if (decision.kind !== 'accept' || AppModule === null) {
    return (
      <>
        <ConnectingOverlay decision={decision} attempts={attempts} />
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
      <App key={hostKey} />
      {signInOverlay}
    </>
  )
}

/** Small, NON-BLOCKING reconnect indicator shown over the still-live view
 *  while a previously-connected REMOTE host reconnects (K2 Connect step #4).
 *  Unlike the old full-screen overlay, this:
 *    - does NOT cover the app or the top-bar (a bottom-center pill),
 *    - passes ALL input through (`pointerEvents: 'none'`) so the app — and
 *      the top-bar host switcher — stay fully usable while we retry.
 *  It only mounts after the drop has been DEBOUNCED (>= REMOTE_DROP_THRESHOLD
 *  consecutive failed polls), so a single blip over a higher-latency tunnel
 *  never flashes it while the data WS is still streaming. */
function ReconnectBanner({ label }: { label: string }): React.ReactElement {
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
        }}
      >
        {/* Amber status dot — "degraded, not down". */}
        <span
          aria-hidden
          style={{
            width: '8px',
            height: '8px',
            borderRadius: '50%',
            background: '#f5a623',
            flexShrink: 0,
          }}
        />
        <span>Reconnecting to {label}… retrying automatically</span>
      </div>
    </div>
  )
}

interface ConnectingOverlayProps {
  decision: GateDecision
  attempts: number
}

function ConnectingOverlay({ decision, attempts }: ConnectingOverlayProps): React.ReactElement {
  const migrating = decision.kind === 'migrating'

  // Heading + sub-line. While the (correct) daemon is migrating we tell
  // the user updates are being applied; otherwise it's a plain connect.
  const heading = migrating ? 'Setting up K2…' : 'Connecting…'
  const subline = migrating
    ? (decision.detail && decision.detail.length > 0 ? decision.detail : 'Applying updates…')
    : null

  // Reload escape hatch. A big upgrade's migration sweep can legitimately
  // take a while, so don't nag during 'migrating' (only after ~60s). For
  // a plain 'wait' (unreachable / wrong version) surface it after ~10s.
  const showReloadButton = migrating ? attempts >= 120 : attempts >= 20

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
            {migrating
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

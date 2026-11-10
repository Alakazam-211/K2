// Pure decision logic + phase handling + copy for the "Update host"
// control (P4 of the remote-update system), split out of GeneralSection.tsx
// so the gating rules, update-phase state machine, and host-named copy are
// unit-testable WITHOUT importing the React component (which transitively
// pulls in Tauri/window-touching modules that can't load in the repo's
// `node` vitest env). GeneralSection's UpdateHostRow renders THROUGH these
// helpers so the UI and the tests can never drift.
//
// This MIRRORS restart-host.ts exactly — same remote-only gating, same
// owner/admin role rules, same host-named copy discipline. The control acts
// on the REMOTE machine you're connected to over K2 Connect, NEVER this Mac
// (the local Tauri auto-updater owns the local update path in GeneralSection).

export type UpdateRole = 'owner' | 'admin' | 'member'

/**
 * The phases the daemon update job moves through, per the P3 status route:
 *   GET /cli/daemon/update/status?job_id= → { phase, progress?, bytes?, error? }
 * phase ∈ downloading | verifying | staged | applying | restarting | done | failed | rolled-back
 */
export type UpdatePhase =
  | 'downloading'
  | 'verifying'
  | 'staged'
  | 'applying'
  | 'restarting'
  | 'done'
  | 'failed'
  | 'rolled-back'

/** The check route's response shape (P3): POST /cli/daemon/update/check. */
export interface UpdateCheckResult {
  current: string
  latest: string
  available: boolean
  notes?: string
  url?: string
  /** 0.39.35: how the remote host is installed. "bundled-app" hosts update
   *  via their co-located Tauri app (Shape A — auto-installs + relaunches,
   *  no separate manual "Install & restart" step); "standalone" hosts via
   *  the in-daemon binary swap (Shape B). Used only to vary copy; the daemon
   *  routes on it server-side. May be absent on older hosts. */
  installKind?: 'standalone' | 'bundled-app' | 'package' | 'unknown'
  /** 0.46.3: a newer version exists but the host's K2 came from a package
   *  manager (`installKind: "package"`, e.g. the Arch package): it can't
   *  update itself, so `available` is false. Absent on older hosts. */
  newerViaPackage?: boolean
  /** 0.46.3: when the manifest behind this answer was fetched (unix s). */
  checkedAt?: number
  /** B4: TRUE when a strictly-newer version exists but there's no published
   *  artifact for this host's platform (a standalone host whose `os-arch`
   *  wasn't built). DISTINCT from "up to date" — `available` is false, but
   *  the host is NOT current. Absent (treated as false) on older hosts. */
  newerNoArtifact?: boolean
  /** B4: this host's platform key (`os-arch`, e.g. `linux-aarch64`) — names
   *  the missing build in the `newerNoArtifact` message. May be absent on
   *  older hosts. */
  platform?: string
}

/** The status route's response shape (P3). */
export interface UpdateStatusResult {
  phase: UpdatePhase
  progress?: number
  bytes?: number
  error?: string | null
  target_version?: string
  /** 0.46.3: the version the job started from (absent on older hosts). */
  from_version?: string
  /** 0.46.3: the version the answering daemon runs (absent on older hosts). */
  running_version?: string
}

// ── Reading the host's replies (z3.k2.dev, 2026-11-05) ───────────────
//
// Rosson updated z3 from his desktop: the row showed "vundefined" and the
// update sat `staged` on the host with no apply. A 2xx whose body is not
// the route's answer (HTML, a bare string, another route's JSON) used to be
// cast straight to the result type, so `current` read as undefined and a
// status without `phase` silently dropped the row back to idle. Every
// update reply now goes through one of these readers: a body of the wrong
// shape is an `UpdateReplyError` that says what came back.

const UPDATE_PHASES: ReadonlySet<string> = new Set<UpdatePhase>([
  'downloading',
  'verifying',
  'staged',
  'applying',
  'restarting',
  'done',
  'failed',
  'rolled-back',
])

/** A host answered an update route with a body that is not that route's
 *  answer. Not a refusal and not a dropped connection. */
export class UpdateReplyError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'UpdateReplyError'
  }
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

function isNonEmptyString(v: unknown): v is string {
  return typeof v === 'string' && v.trim() !== ''
}

/** The first ~80 characters of a reply, on one line, for an error message. */
export function replySnippet(body: unknown): string {
  if (body === undefined || body === null || body === '') return 'an empty reply'
  let raw: string
  if (typeof body === 'string') {
    raw = body
  } else {
    try {
      raw = JSON.stringify(body) ?? String(body)
    } catch {
      raw = String(body)
    }
  }
  const flat = raw.replace(/\s+/g, ' ').trim()
  return `"${flat.length > 80 ? `${flat.slice(0, 80)}…` : flat}"`
}

/** `POST daemon/update/check`'s answer, or an `UpdateReplyError`. Every
 *  host with the route (0.39.33+) sends `current`, `latest`, `available`. */
export function readUpdateCheck(hostLabel: string, body: unknown): UpdateCheckResult {
  if (
    isRecord(body) &&
    isNonEmptyString(body.current) &&
    isNonEmptyString(body.latest) &&
    typeof body.available === 'boolean'
  ) {
    return body as unknown as UpdateCheckResult
  }
  throw new UpdateReplyError(
    `Unexpected reply from ${hostLabel} to the update check (expected its current and latest version): got ${replySnippet(body)}`,
  )
}

/** `GET daemon/update/status`'s answer, or an `UpdateReplyError`. */
export function readUpdateStatus(hostLabel: string, body: unknown): UpdateStatusResult {
  if (isRecord(body) && typeof body.phase === 'string' && UPDATE_PHASES.has(body.phase)) {
    return body as unknown as UpdateStatusResult
  }
  throw new UpdateReplyError(
    `Unexpected reply from ${hostLabel} to the update status (expected a phase): got ${replySnippet(body)}`,
  )
}

/** `POST daemon/update/start`'s answer (`{job_id}`), or an `UpdateReplyError`. */
export function readUpdateStart(hostLabel: string, body: unknown): { job_id: string } {
  if (isRecord(body) && isNonEmptyString(body.job_id)) return { job_id: body.job_id }
  throw new UpdateReplyError(
    `Unexpected reply from ${hostLabel} to the update start (expected a job id): got ${replySnippet(body)}`,
  )
}

// ── The downloaded update survives leaving the page ──────────────────
//
// Settings → General's update is two steps (Download, then Install &
// restart), and the job id lived only in the row's state: leaving the page
// after Download lost it, so a staged update had no Install button and
// Check → Download started a second download. The row keeps its job here,
// per host id, for the life of this window, and picks it up when it mounts.

export interface KeptUpdateJob {
  jobId: string
  /** The check that offered this update (versions + installKind). */
  check: UpdateCheckResult
  /** `apply` was sent: a dropped host now means it is installing. */
  applySent: boolean
}

const keptUpdateJobs = new Map<string, KeptUpdateJob>()

export function keepUpdateJob(hostId: string, job: KeptUpdateJob): void {
  keptUpdateJobs.set(hostId, { ...job })
}

export function keptUpdateJob(hostId: string): KeptUpdateJob | null {
  const job = keptUpdateJobs.get(hostId)
  return job ? { ...job } : null
}

export function forgetUpdateJob(hostId: string): void {
  keptUpdateJobs.delete(hostId)
}

export function forgetAllUpdateJobsForTests(): void {
  keptUpdateJobs.clear()
}

/** The row's line when the kept job is gone from the host (it restarted
 *  before Install & restart): nothing was installed. */
export function updateJobGoneCopy(hostLabel: string): string {
  return `${hostLabel} no longer has the downloaded update (it restarted), so nothing was installed. Check for updates to download it again.`
}

/** The row's line while status replies can't be read. */
export function updateStatusUnreadableCopy(message: string): string {
  return `${message}. Still checking…`
}

/**
 * Visibility/enablement decision for the Update-host control by a resolved
 * role (identical rules to restartHostVisibility). Settings → General now
 * gates on `hostActionGate` (host-role.ts), which also handles a role that
 * is not read yet; this stays for the older role-only callers/tests:
 *   - LOCAL active host → never shown (the local-Mac update is the Tauri
 *     auto-updater "App Version" area).
 *   - remote without the route (serverSupports=false) → hidden.
 *   - Member viewer → hidden (the owner-gated route would 403).
 *   - Owner/Admin → shown + enabled. Unknown role (whoami pending/failed)
 *     → shown but DISABLED (button inert until a role resolves).
 */
export function updateHostVisibility(args: {
  isRemote: boolean
  supportsUpdate: boolean
  role: UpdateRole | null
}): { show: boolean; canUpdate: boolean } {
  const { isRemote, supportsUpdate, role } = args
  if (!isRemote) return { show: false, canUpdate: false }
  if (!supportsUpdate) return { show: false, canUpdate: false }
  if (role === 'member') return { show: false, canUpdate: false }
  return { show: true, canUpdate: role === 'owner' || role === 'admin' }
}

/** True once the staged build is ready to install & restart the host. */
export function isStaged(phase: UpdatePhase | null): boolean {
  return phase === 'staged'
}

/** True for terminal phases that should stop the status poll loop. The host
 *  goes away on `restarting`/`done` (the ConnectionGate's soft-reconnect
 *  takes over), and `failed`/`rolled-back` are dead ends — none warrant
 *  further polling. `staged` is intentionally NOT terminal: Shape B hosts
 *  still need a client `apply` after stage, and Shape A keeps polling until
 *  the co-located app drives restart. */
export function isTerminalPhase(phase: UpdatePhase | null): boolean {
  return (
    phase === 'done' ||
    phase === 'restarting' ||
    phase === 'failed' ||
    phase === 'rolled-back'
  )
}

/** Shape B (standalone): after start, when status hits `staged`, client MUST apply.
 *  Shape A (bundled-app): start alone is enough — never call apply. */
export function shouldAutoApplyAfterStage(installKind: string | undefined): boolean {
  return installKind !== 'bundled-app'
}

/** True for the two failure phases that must be surfaced as a host-named
 *  error (the host stayed/rolled back on its old version). */
export function isFailurePhase(phase: UpdatePhase | null): boolean {
  return phase === 'failed' || phase === 'rolled-back'
}

/**
 * The progress/phase line shown on the row while an update job runs. NAMES
 * the host (the unmistakable-remote requirement) and renders the daemon's
 * phase verbatim into plain language, threading `progress` for the
 * download. `failed`/`rolled-back` are framed as the host staying on its
 * current version.
 */
export function updatePhaseCopy(
  phase: UpdatePhase,
  hostLabel: string,
  opts?: { progress?: number; current?: string },
): string {
  switch (phase) {
    case 'downloading': {
      const pct =
        typeof opts?.progress === 'number' ? ` (${Math.round(opts.progress)}%)` : ''
      return `Downloading update for ${hostLabel}…${pct}`
    }
    case 'verifying':
      return `Verifying update for ${hostLabel}…`
    case 'staged':
      return `Update staged for ${hostLabel} — ready to install & restart.`
    case 'applying':
      return `Installing update on ${hostLabel}…`
    case 'restarting':
      return `Installing & restarting ${hostLabel}… it'll reconnect automatically.`
    case 'done':
      return `${hostLabel} updated and is reconnecting.`
    case 'failed':
      return `Update failed — ${hostLabel} is still on ${opts?.current ?? 'its current version'}.`
    case 'rolled-back':
      return `Update rolled back — ${hostLabel} is still on ${opts?.current ?? 'its current version'}.`
  }
}

/**
 * Copy for the comeback watcher's RESOLVED state (0.40.48 remote-update
 * ergonomics). `restarting` used to be a dead end: the status poll stops
 * (the host is going away) and the row said "Installing & restarting …
 * it'll reconnect automatically." FOREVER — even long after the reconnect
 * succeeded. The row now polls the host's public /boot-status after
 * `restarting`/`done` and lands here when `phase === 'ready'`:
 *   - back on the EXPECTED version → plain success;
 *   - back on a DIFFERENT version → says so (the update may have rolled
 *     back server-side — never claim success we didn't verify);
 *   - version unreadable (older daemon omits it) → success without a number.
 */
export function updateCompleteCopy(
  hostLabel: string,
  expected: string | undefined,
  bootVersion: string | undefined,
): string {
  if (bootVersion && expected && bootVersion !== expected) {
    return `${hostLabel} is back — but on v${bootVersion}, not v${expected}. The update may have rolled back.`
  }
  const v = bootVersion ?? expected
  return v
    ? `${hostLabel} updated to v${v} and reconnected.`
    : `${hostLabel} is back and reconnected.`
}

/** Copy while the comeback watcher has waited past its soft deadline —
 *  honest "still working on it", not a spinner and not a false failure. */
export function updateSlowComebackCopy(hostLabel: string): string {
  return `${hostLabel} is taking longer than expected to come back — it may still be installing. Still watching…`
}

/**
 * Comeback watcher verdict for post-apply /boot-status polling.
 *   - keep-watching: not ready yet, or ready on the OLD version without ever
 *     seeing the host go down (pre-update daemon still answering — Baden bug)
 *   - success: ready on the expected version (or ready after sawDown with no
 *     usable version pair to contradict)
 *   - wrong-version: ready after sawDown, but version ≠ expected — update
 *     did not take / rolled back. Callers must NOT claim update success.
 */
export type ComebackVerdict = 'keep-watching' | 'success' | 'wrong-version'

/**
 * The comeback watcher's RESOLUTION RULE (pure, unit-tested — the Baden
 * false-rollback bug): during an update the OLD daemon keeps answering
 * /boot-status `ready` right up until the process actually swaps
 * (bundled-app hosts especially — the Tauri updater downloads for a while
 * before relaunching). Resolving on the FIRST `ready` therefore reads the
 * PRE-update daemon and mis-reports "rolled back to <old>" for an update
 * that succeeds seconds later.
 *
 * Resolve ONLY when there's evidence the restart actually happened:
 *   - the reported version EQUALS the expected one (the update took), OR
 *   - the watcher saw the host go DOWN at least once (`sawDown`) and it's
 *     now back `ready` — whatever version it reports is then the honest
 *     post-restart truth (a real rollback surfaces as wrong-version).
 * A `ready` probe with neither is the old daemon still running: keep
 * watching.
 *
 * Returns a 3-way verdict so Connections can hard-fail wrong-version while
 * General (and anyone using truthiness) can still treat both success and
 * wrong-version as "resolved" via `!== 'keep-watching'`.
 */
export function shouldResolveComeback(args: {
  phase: string | undefined
  version: string | undefined
  expected: string | undefined
  sawDown: boolean
}): ComebackVerdict {
  if (args.phase !== 'ready') return 'keep-watching'
  if (args.version && args.expected && args.version === args.expected) return 'success'
  if (!args.sawDown) return 'keep-watching'
  // Host went down and is back. Version mismatch ⇒ the update did not take.
  if (args.version && args.expected && args.version !== args.expected) {
    return 'wrong-version'
  }
  // sawDown + ready with no comparable version pair (older daemon omits
  // version, or expected was lost) — honest post-restart truth, numberless.
  return 'success'
}

// ── Remote update across the restart (0.46.3) ─────────────────────────
//
// Rosson 2026-10-19: updating a remote server from Settings → General always
// said it failed, yet the server updated. On a Linux host without hand-off
// the daemon stopped its K2 Connect tunnel BEFORE answering `apply`, so the
// ack never arrived; the client saw "Load failed", retried `apply` once into
// the dead tunnel, and toasted "Couldn't update <host>: Load failed". A
// restarting daemon cutting the connection is the update WORKING, not
// failing. The rules below decide from what the host reports once it is
// back: its `/boot-status` version + instanceId, and (0.46.3 daemons) the
// update job, which now survives the restart with the real verdict.

/** HTTP status on a daemon/host-op error, when it carries one. */
function errorStatus(err: unknown): number | undefined {
  const s = (err as { status?: unknown } | null)?.status
  return typeof s === 'number' ? s : undefined
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

/** Connection-level failure (dead socket, refused, CORS-masked edge 404) —
 *  same matcher as lib/remote-retry's `isConnectionLevelError`, kept local
 *  so this module stays dependency-free for the node test env. */
function looksLikeConnectionDrop(err: unknown): boolean {
  if (!(err instanceof Error)) return false
  const m = err.message.toLowerCase()
  return (
    m.includes('failed to fetch') ||
    m.includes('load failed') ||
    m.includes('networkerror') ||
    m.includes('access-control-allow-origin') ||
    m.includes('not allowed by access-control') ||
    m.includes('connection refused') ||
    m.includes('econnrefused') ||
    m.includes('daemon not reachable') ||
    m.includes('daemon_ws_url invoke failed') ||
    m.includes('timed out') ||
    m.includes('timeout') ||
    err.name === 'TimeoutError' ||
    err.name === 'AbortError'
  )
}

/** Relay/edge answers while the host's tunnel or daemon is down (502/503/504),
 *  and the daemon's own 503 while it boots. */
function isGatewayStatus(status: number | undefined): boolean {
  return status === 502 || status === 503 || status === 504
}

/**
 * After `apply` was SENT, does this error leave the outcome open (the host may
 * well be restarting onto the new version) rather than prove `apply` was
 * refused? True for:
 *   - a dropped connection / timeout (the daemon or its tunnel went away
 *     mid-answer),
 *   - a gateway 502/503/504 (the relay has no tunnel to the host right now),
 *   - a 400 saying the job is no longer `staged` or is unknown: a retried
 *     `apply` that reached the NEW daemon (the first one went through).
 * A 403, a 500 "swap binary: …" / "back up running binary: …" etc. are real
 * refusals from a daemon that is still up on the old version → false.
 */
export function isAmbiguousApplyError(err: unknown): boolean {
  if (isForbiddenError(errorText(err))) return false
  if (looksLikeConnectionDrop(err)) return true
  const status = errorStatus(err)
  if (isGatewayStatus(status)) return true
  if (status === 400 || status === undefined) {
    return /not 'staged'|unknown job_id/i.test(errorText(err))
  }
  return false
}

/** A 400 from `apply` saying the job is unknown or no longer `staged`. Only
 *  a RETRIED apply that reached the new daemon makes that the install going
 *  ahead; a first answer from the same daemon means nothing was installed.
 *  `settleApplyRefusal` tells the two apart. */
export function isJobStateRefusal(err: unknown): boolean {
  const status = errorStatus(err)
  if (status !== 400 && status !== undefined) return false
  return /not 'staged'|unknown job_id/i.test(errorText(err))
}

export type ApplyRefusalVerdict =
  /** The host is installing (or did): watch it come back. */
  | { kind: 'went-ahead' }
  /** The same daemon is still up and the job never left `staged` (or is
   *  gone): nothing was installed. */
  | { kind: 'refused'; message: string; jobGone: boolean }

const STARTED_PHASES: ReadonlySet<string> = new Set([
  'applying',
  'restarting',
  'done',
  'failed',
  'rolled-back',
])

/**
 * `apply` answered 400 "unknown job_id …" or "… not 'staged'". Settings →
 * General used to treat that as the install going ahead and waited up to
 * ten minutes for a restart that never came (Rosson 2026-11-05: "Install &
 * restart … doesn't actually perform the update"). Decide from what the
 * host reports now:
 *   - unreachable, or a different daemon instance than before `apply`
 *     → it restarted: went ahead;
 *   - the job says applying / restarting / done / failed / rolled-back
 *     → went ahead (the comeback watcher reports the verdict);
 *   - the job is still downloading / verifying / staged → refused;
 *   - the job is unknown on the SAME instance → refused (the host restarted
 *     after the download and lost it); with no instance ids to compare it
 *     can't be proved, so it is watched as before.
 */
export async function settleApplyRefusal(deps: {
  hostLabel: string
  err: unknown
  /** `/boot-status` instanceId read just before `apply`. */
  startInstanceId: string | undefined
  bootStatus: () => Promise<{ instanceId?: string } | null>
  /** The job now: its status, `'unknown'` for a 404, null when unreadable. */
  jobStatus: () => Promise<UpdateStatusResult | 'unknown' | null>
}): Promise<ApplyRefusalVerdict> {
  const boot = await deps.bootStatus()
  if (boot === null) return { kind: 'went-ahead' }
  const idsComparable = !!deps.startInstanceId && !!boot.instanceId
  if (idsComparable && boot.instanceId !== deps.startInstanceId) return { kind: 'went-ahead' }
  const job = await deps.jobStatus()
  const notInstalled = `${deps.hostLabel} did not install the update: ${errorText(deps.err)}`
  if (job !== null && job !== 'unknown') {
    if (STARTED_PHASES.has(job.phase)) return { kind: 'went-ahead' }
    return { kind: 'refused', message: notInstalled, jobGone: false }
  }
  if (!idsComparable) return { kind: 'went-ahead' }
  if (job === 'unknown') return { kind: 'refused', message: updateJobGoneCopy(deps.hostLabel), jobGone: true }
  return { kind: 'refused', message: notInstalled, jobGone: false }
}

/**
 * While polling `status?job_id=` after `start`, does this error mean the host
 * is going away to install (watch for its comeback) rather than a blip to
 * ignore? Only once the host may legitimately restart:
 *   - standalone (Shape B): after `apply` was sent;
 *   - bundled-app (Shape A): any time after `start` — the co-located app
 *     posts `applying`/`restarting` and stops the daemon within milliseconds,
 *     so a 1.5–2 s poll usually never sees those phases.
 * The error must be a connection drop, a gateway status, or a 404 for the
 * job (a pre-0.46.3 daemon that restarted and forgot it). A 403 never is.
 */
export function pollErrorMeansRestart(args: {
  err: unknown
  installKind: string | undefined
  applySent: boolean
}): boolean {
  const { err, installKind, applySent } = args
  if (!applySent && installKind !== 'bundled-app') return false
  if (isForbiddenError(errorText(err))) return false
  if (looksLikeConnectionDrop(err)) return true
  const status = errorStatus(err)
  return isGatewayStatus(status) || status === 404
}

/** How a Settings → Connections update job ended. */
export type UpdateJobOutcome =
  /** The job reported `restarting`/`done`. */
  | { kind: 'applied' }
  /** The host went away after `apply` (or a Shape A start): watch it come back. */
  | { kind: 'unreachable' }
  /** The job itself failed or rolled back on the host. */
  | { kind: 'job-failed'; status: UpdateStatusResult }
  /** A status poll failed before `apply` (refusal, dropped host, unreadable reply). */
  | { kind: 'poll-failed'; err: unknown }
  /** `apply` was refused by a host that is still up. */
  | { kind: 'apply-failed'; err: unknown }
  /** The job never reached a verdict in the poll window. */
  | { kind: 'timed-out' }

export interface UpdateJobDeps {
  hostLabel: string
  installKind: string | undefined
  /** `GET daemon/update/status?job_id=` — the raw body; read here. */
  status: () => Promise<unknown>
  /** `POST daemon/update/apply {job_id}`. */
  apply: () => Promise<unknown>
  sleep: (ms: number) => Promise<void>
  /** Each status the host reported (for the progress line). */
  onStatus?: (s: UpdateStatusResult) => void
  /** Just before `apply` is sent. */
  onApplying?: () => void
  intervalMs?: number
  maxPolls?: number
}

/**
 * Drive a started update job to an outcome (Settings → Connections, where
 * the person confirmed "Install & restart" before `start`). Shape B
 * (standalone / unknown): when the job is `staged`, send `apply` once — the
 * binary never swaps without it (#55 Bug A). Shape A (bundled-app): never
 * apply; the co-located app installs itself.
 *
 * It does not stop when the tile unmounts: the person already confirmed the
 * install, and stopping left the job `staged` on the host with nothing to
 * apply it (z3.k2.dev, 2026-11-05). A status reply it can't read ends the
 * job with that error before `apply`, never a 3-minute wait.
 */
export async function driveUpdateJob(deps: UpdateJobDeps): Promise<UpdateJobOutcome> {
  const interval = deps.intervalMs ?? 2000
  const maxPolls = deps.maxPolls ?? 90
  const kind = deps.installKind
  let applySent = false
  let sawStaged = false
  for (let i = 0; i < maxPolls; i++) {
    await deps.sleep(interval)
    let status: UpdateStatusResult
    try {
      status = readUpdateStatus(deps.hostLabel, await deps.status())
    } catch (err) {
      // After apply (or after a Shape A stage, when the app is installing)
      // the host going away is the install. Before that, a failed or
      // unreadable poll is a real failure — never claim it is restarting.
      if (
        applySent ||
        (sawStaged && !shouldAutoApplyAfterStage(kind)) ||
        (!(err instanceof UpdateReplyError) &&
          pollErrorMeansRestart({ err, installKind: kind, applySent }))
      ) {
        return { kind: 'unreachable' }
      }
      return { kind: 'poll-failed', err }
    }
    deps.onStatus?.(status)
    if (isFailurePhase(status.phase)) return { kind: 'job-failed', status }
    if (isStaged(status.phase)) {
      sawStaged = true
      if (shouldAutoApplyAfterStage(kind) && !applySent) {
        applySent = true
        deps.onApplying?.()
        try {
          await deps.apply()
        } catch (err) {
          // Apply often races the host drop: a connection error, a gateway
          // 502/503/504, or a retried apply that reached the NEW daemon is
          // the install going ahead, not a failure.
          if (isAmbiguousApplyError(err) && !isAuthError(errorText(err))) {
            return { kind: 'unreachable' }
          }
          return { kind: 'apply-failed', err }
        }
        continue
      }
      // Shape A: keep polling until terminal / the host goes away.
    }
    if (status.phase === 'restarting' || status.phase === 'done') return { kind: 'applied' }
  }
  return { kind: 'timed-out' }
}

/** The comeback watcher's outcome. `unconfirmed` is NOT a failure: the host
 *  never showed us a verdict within the window (still installing, or its
 *  version can't be compared). */
export type UpdateComebackOutcome =
  | { kind: 'success'; version?: string }
  | { kind: 'failed'; reason: string; version?: string }
  | { kind: 'unconfirmed'; reason: string; version?: string }

/** What the watcher reads from the host each tick. Both return `null` when
 *  the host is unreachable / the job is unknown — never throw. */
export interface UpdateComebackDeps {
  /** The host's public `/boot-status` (no token). */
  bootStatus: () => Promise<{
    phase?: string
    ready?: boolean
    version?: string
    instanceId?: string
  } | null>
  /** `GET daemon/update/status?job_id=` — the persisted verdict on 0.46.3+. */
  jobStatus: () => Promise<UpdateStatusResult | null>
  sleep: (ms: number) => Promise<void>
  now: () => number
  /** False once the row unmounts / the host switches: stop quietly. */
  alive: () => boolean
  /** Called once when the soft deadline passes (copy turns "taking longer"). */
  onSlow?: () => void
}

export interface UpdateComebackOptions {
  /** The version the update should land on (`check.latest`). */
  expected?: string
  /** The version the host was on before (`check.current`). */
  from?: string
  /** `/boot-status` instanceId read before the restart was triggered. A
   *  different id afterwards proves the daemon restarted. */
  startInstanceId?: string
  intervalMs?: number
  slowAfterMs?: number
  giveUpAfterMs?: number
}

/** Pre-restart phases: a job still in one of these was never restarted. */
const PRE_RESTART_PHASES: ReadonlySet<string> = new Set([
  'downloading',
  'verifying',
  'staged',
  'applying',
])

/**
 * Watch a remote host come back from an update and decide the outcome
 * (0.46.3). Polls until one of:
 *   - success: the host is `ready` on the expected version (or, with no
 *     comparable version, it restarted and its job says `done`);
 *   - failed: the job reports `failed`/`rolled-back` (its real reason), or the
 *     host restarted and came back on a different version;
 *   - unconfirmed: the give-up deadline passed without either.
 * Returns `null` if `alive()` turned false (unmounted / host switched).
 *
 * "Restarted" = the instanceId changed when both ids are known (a network
 * blip that never restarted the daemon does not count), else the host was
 * seen down at least once (the pre-instanceId rule, `shouldResolveComeback`).
 */
export async function watchUpdateComeback(
  deps: UpdateComebackDeps,
  opts: UpdateComebackOptions,
): Promise<UpdateComebackOutcome | null> {
  const interval = opts.intervalMs ?? 2500
  const start = deps.now()
  const slowAt = start + (opts.slowAfterMs ?? 4 * 60_000)
  const giveUpAt = start + (opts.giveUpAfterMs ?? 10 * 60_000)
  const { expected, from, startInstanceId } = opts
  let sawDown = false
  let slowFired = false
  let lastReady: { version?: string; restarted: boolean } | null = null

  while (deps.alive() && deps.now() < giveUpAt) {
    await deps.sleep(interval)
    if (!deps.alive()) return null
    const boot = await deps.bootStatus()
    if (!deps.alive()) return null
    if (boot === null) {
      sawDown = true
    } else if (boot.phase === 'ready' || boot.ready === true) {
      const idsComparable = !!startInstanceId && !!boot.instanceId
      const restarted = idsComparable ? boot.instanceId !== startInstanceId : sawDown
      const version = boot.version
      lastReady = { version, restarted }

      // The daemon's own verdict wins when it has one (0.46.3+ persists the
      // job across the restart; older daemons 404 → null).
      const job = await deps.jobStatus()
      if (!deps.alive()) return null
      if (job && isFailurePhase(job.phase)) {
        return {
          kind: 'failed',
          reason: job.error || `the update ${job.phase === 'rolled-back' ? 'was rolled back' : 'failed'}`,
          version,
        }
      }
      if (job && job.phase === 'done' && restarted) {
        return { kind: 'success', version: version ?? job.running_version }
      }

      const verdict = shouldResolveComeback({
        phase: 'ready',
        version,
        expected,
        sawDown: restarted,
      })
      if (verdict === 'success') return { kind: 'success', version }
      if (verdict === 'wrong-version') {
        // The daemon is still settling (Linux boot verifier counting) or the
        // "down" was a blip and the job never restarted: keep watching.
        if (job && (job.phase === 'restarting' || PRE_RESTART_PHASES.has(job.phase))) {
          if (!idsComparable && PRE_RESTART_PHASES.has(job.phase)) sawDown = false
        } else {
          return {
            kind: 'failed',
            reason:
              version && version === from
                ? `it came back on v${version}; the update to v${expected} did not install`
                : `it came back on v${version ?? 'an unknown version'}, not v${expected}`,
            version,
          }
        }
      }
    }
    if (!slowFired && deps.now() >= slowAt) {
      slowFired = true
      deps.onSlow?.()
    }
  }
  if (!deps.alive()) return null
  const mins = Math.round(((opts.giveUpAfterMs ?? 10 * 60_000) / 60_000) * 10) / 10
  if (lastReady && !lastReady.restarted) {
    return {
      kind: 'unconfirmed',
      reason: `it is still answering on v${lastReady.version ?? 'its old version'} and has not restarted yet`,
      version: lastReady.version,
    }
  }
  if (lastReady) {
    return {
      kind: 'unconfirmed',
      reason: `it restarted but has not confirmed v${expected ?? 'the new version'} yet (it reports v${lastReady.version ?? '?'})`,
      version: lastReady.version,
    }
  }
  return { kind: 'unconfirmed', reason: `it did not come back within ${mins} minutes` }
}

/** In-flight line while the host restarts onto the update. */
export function updateReconnectingCopy(hostLabel: string): string {
  return `Updating ${hostLabel}… reconnecting…`
}

/** Verified success line. */
export function updateSucceededCopy(hostLabel: string, version: string | undefined): string {
  return version ? `Updated ${hostLabel} to v${version}.` : `${hostLabel} updated and is back.`
}

/** A real failure, with the host's own reason. */
export function updateFailedCopy(hostLabel: string, reason: string): string {
  return `Update failed on ${hostLabel}: ${reason}.`.replace(/\.\.$/, '.')
}

/** The watcher gave up without a verdict — NOT a failure. */
export function updateUnconfirmedCopy(hostLabel: string, reason: string): string {
  return `Couldn't confirm the update on ${hostLabel}: ${reason}. Check for updates to see its version.`
}

/** Copy for the "Update available — <current> → <latest>" banner once a
 *  check reports `available`. NAMES the host. */
export function updateAvailableCopy(
  hostLabel: string,
  current: string,
  latest: string,
): string {
  return `Update available for ${hostLabel} — ${current} → ${latest}`
}

/** B1: The persistent host-version line shown after ANY successful check —
 *  for BOTH the up-to-date and update-available states — so the remote
 *  host's CURRENT version is always visible (never just inside a transient
 *  toast or the update-available banner). NAMES the host. */
export function hostVersionCopy(hostLabel: string, current: string): string {
  return `${hostLabel} is on v${current}`
}

/** B4: a strictly-newer version exists but there's no build for this host's
 *  platform — DISTINCT from "up to date". NAMES the host, the newer version,
 *  and the platform whose build is missing. */
export function newerNoArtifactCopy(
  hostLabel: string,
  latest: string,
  platform: string | undefined,
): string {
  const plat = platform && platform.trim() ? platform : 'this platform'
  return `A newer version (v${latest}) exists, but there's no build for ${hostLabel}'s platform (${plat}). Nothing to install yet.`
}

/** 0.46.3: a newer version exists but the host's K2 came from a package
 *  manager, so it updates there, not from here. */
export function newerViaPackageCopy(hostLabel: string, latest: string): string {
  return `A newer version (v${latest}) is out. ${hostLabel}'s K2 was installed with a package manager, so update it there (for example \`sudo pacman -Syu k2\`).`
}

/** The confirm-dialog copy for the install-&-restart step — it NAMES the
 *  active host (the unmistakable-remote requirement), framed as the REMOTE
 *  machine, explicitly NOT this Mac. */
export function updateHostConfirmCopy(
  hostLabel: string,
  hostname: string,
  latest: string,
): {
  title: string
  message: string
  confirmLabel: string
} {
  return {
    title: `Update ${hostLabel} to ${latest}?`,
    message:
      `This installs ${latest} on ${hostLabel} (${hostname}), the server you're connected to, not this computer, and restarts it. ` +
      `Sessions on it disconnect briefly, then reconnect automatically when it's back on the new version.`,
    confirmLabel: `Install & restart ${hostLabel}`,
  }
}

/** A host-named message for a 403 from the update routes when the role
 *  can't be read again (Settings → Connections). The daemon's 403 body is
 *  the same for a Member and for a login it no longer knows, so it names
 *  both; Settings → General reads whoami and says which (`refusalCopy`). */
export function updateForbiddenCopy(hostLabel: string): string {
  return `${hostLabel} turned down the update. Your sign-in there may have expired (sign in again), or your login isn't the server's owner or an admin.`
}

/** True when an error message looks like a 403/forbidden from the
 *  owner-token-gated update routes (same matcher the restart row uses). */
export function isForbiddenError(message: string): boolean {
  return /403|forbidden|invalid or missing token/i.test(message)
}

/** True when a per-host op failed because the session token is stale/rejected
 *  (401-class) — the tile should drop the dead token and offer re-sign-in.
 *  NOTE: a network failure ("Load failed", timeouts) is NOT an auth error — the
 *  server is unreachable, the token may still be fine, so we don't clear it. */
export function isAuthError(message: string): boolean {
  return /\b401\b|unauthorized|session (?:expired|invalid|not found)|token (?:expired|invalid|rejected)|not signed in|please sign in/i.test(
    message,
  )
}


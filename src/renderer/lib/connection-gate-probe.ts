// Module-level hooks for ConnectionGate's effect-local soft health poll.
//
// R1 (WS-driven recovery) must couple entry with exit: setRecovery(reconnecting)
// alone would leave banner/cliFetch latched until the next 25s tick. The gate
// effect registers a force-probe twin of the visibility handler (cancel timer
// + tick now) and a flap-stamp callback so R1 can arm GH#57 flap detection
// without lifting the whole poll machine into the store.
//
// Store-free so remote-ws-drop / session-events can import without cycles.

type RemoteHealthControls = {
  forceProbe: () => void
  stampFlap: () => void
}

let controls: RemoteHealthControls | null = null

/** True after R1 stamped flap for the current drop episode; soft-poll fail
 *  skips a second stamp for the same episode (D9). Cleared on soft accept /
 *  consecutiveFails reset. */
let r1FlapStampedThisEpisode = false

export function registerRemoteHealthControls(
  next: RemoteHealthControls | null,
): void {
  controls = next
  // A new gate run (host switch / remount) starts with no evidence: a
  // failure reported against the previous host must not count here.
  if (next !== null) pendingFailureEvidence = false
  if (next === null) {
    // Effect cleanup — leave episode flag alone so an in-flight R1 stamp
    // still suppresses a soft-poll double-stamp if the effect re-runs mid
    // episode; soft accept / host switch clears explicitly.
  }
}

/**
 * Failure evidence (0.46.10): a request or socket on the window's remote
 * host failed at the connection level since the last health tick. It is
 * never a verdict on its own. A one-off failed request on a healthy host
 * (WebKit reports a dropped preflight as CORS / "Load failed") used to flip
 * the whole host into recovery or an auto-reload, tearing down every
 * socket. Now it only asks the gate to probe: a healthy `/boot-status`
 * drops the evidence; a failed probe counts the evidence as one strike, so
 * evidence + a failed probe reaches the drop threshold at once (a dead host
 * still goes to recovery promptly). One flag, not a counter: a storm of
 * failed requests is still one strike, and the probe decides.
 */
let pendingFailureEvidence = false

/** Report a connection-level failure against the window's remote host and
 *  probe it now. Use this instead of setting recovery directly. */
export function reportRemoteFailure(): void {
  pendingFailureEvidence = true
  forceSoftHealthProbe()
}

/** Read and clear the evidence (ConnectionGate's tick, after its probe). */
export function takeRemoteFailureEvidence(): boolean {
  const had = pendingFailureEvidence
  pendingFailureEvidence = false
  return had
}

/** Test seam: is evidence waiting for the next tick? */
export function hasRemoteFailureEvidenceForTests(): boolean {
  return pendingFailureEvidence
}

/** Cancel pending health timeout and run ConnectionGate soft `tick()` now.
 *  Coalesces to one tick per turn so a compose/cliFetch CORS storm does
 *  not fan out N arbiter probes. */
let forceProbeQueued = false

export function __resetForceProbeLatchForTests(): void {
  forceProbeQueued = false
  pendingFailureEvidence = false
}

export function forceSoftHealthProbe(): void {
  if (!controls || forceProbeQueued) return
  forceProbeQueued = true
  queueMicrotask(() => {
    forceProbeQueued = false
  })
  if (import.meta.env.DEV) {
    // eslint-disable-next-line no-console
    console.debug('[recovery] forceSoftHealthProbe')
  }
  controls.forceProbe()
}

/**
 * Stamp one reconnect-flap surface for the current R1 episode.
 * Idempotent within the episode: subsequent calls no-op until
 * {@link clearR1FlapEpisode}.
 */
export function stampRemoteReconnectFlap(): void {
  if (r1FlapStampedThisEpisode) return
  r1FlapStampedThisEpisode = true
  controls?.stampFlap()
}

/** Soft-poll fail branch: skip a second flap stamp if R1 already stamped. */
export function wasR1FlapStampedThisEpisode(): boolean {
  return r1FlapStampedThisEpisode
}

/** Clear episode latch on soft accept / consecutiveFails=0 / host switch. */
export function clearR1FlapEpisode(): void {
  r1FlapStampedThisEpisode = false
}

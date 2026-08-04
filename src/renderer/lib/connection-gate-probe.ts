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
  if (next === null) {
    // Effect cleanup — leave episode flag alone so an in-flight R1 stamp
    // still suppresses a soft-poll double-stamp if the effect re-runs mid
    // episode; soft accept / host switch clears explicitly.
  }
}

/** Cancel pending health timeout and run ConnectionGate soft `tick()` now. */
export function forceSoftHealthProbe(): void {
  if (import.meta.env.DEV) {
    // eslint-disable-next-line no-console
    console.debug('[recovery] forceSoftHealthProbe')
  }
  controls?.forceProbe()
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

// Grid-WS health tick (PRD grid-pause-snapshot-hitch G2/G4/G5).
//
// OPEN + silence is healthy idle. Keep the 15s last-version ack probe
// (daemon pause unblock). Do **not** latch a stall, log recovered, or
// forceGridResync('grid-stall-no-frame'). Dead sockets still warn
// `[grid-stall]` with ws-missing / ws-not-open; reattach is the
// ready-ws-not-open poll / input-dead-ws, not this tick.

/** WS readyState: CONNECTING */
export const WS_CONNECTING = 0
/** WS readyState: OPEN */
export const WS_OPEN = 1

export const ACK_PROBE_MS = 15_000
export const STALL_WS_MS = 5_000

export interface GridHealthInput {
  now: number
  /** `null` = no socket (ws-missing). */
  readyState: number | null
  lastFrameAt: number
  lastAckVersion: number
  lastAckProbeAt: number
  stallActive: boolean
}

export interface GridHealthTick {
  /** Re-send this k1 ack version, or null. Probe is allowed on OPEN idle. */
  ackProbeVersion: number | null
  /** Once-per-episode not-OPEN `[grid-stall]` reason, or null. */
  stallWarnReason: string | null
  stallActive: boolean
}

export function tickGridHealth(input: GridHealthInput): GridHealthTick {
  const {
    now,
    readyState,
    lastFrameAt,
    lastAckVersion,
    lastAckProbeAt,
    stallActive,
  } = input
  const ageMs = lastFrameAt > 0 ? Math.round(now - lastFrameAt) : null

  if (readyState === WS_OPEN) {
    const probe =
      ageMs !== null &&
      ageMs >= ACK_PROBE_MS &&
      lastAckVersion > 0 &&
      now - lastAckProbeAt >= ACK_PROBE_MS
        ? lastAckVersion
        : null
    return {
      ackProbeVersion: probe,
      stallWarnReason: null,
      // OPEN never latches a stall (20s OPEN silence is idle).
      stallActive,
    }
  }

  if (readyState === WS_CONNECTING) {
    return { ackProbeVersion: null, stallWarnReason: null, stallActive }
  }
  if (lastFrameAt === 0 && ageMs === null) {
    return { ackProbeVersion: null, stallWarnReason: null, stallActive }
  }
  if (ageMs !== null && ageMs < STALL_WS_MS && lastFrameAt > 0) {
    return { ackProbeVersion: null, stallWarnReason: null, stallActive }
  }

  const reason =
    readyState === null ? 'ws-missing' : `ws-not-open:${readyState}`
  return {
    ackProbeVersion: null,
    stallWarnReason: stallActive ? null : reason,
    stallActive: true,
  }
}

/** Recovered warn is only for a real not-OPEN stall episode, never
 *  healthy OPEN idle (stallActive stays false there). */
export function shouldLogGridStallRecovered(stallActive: boolean): boolean {
  return stallActive
}

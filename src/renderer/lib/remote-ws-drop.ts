// R1 — WS-driven recovery from session-events drops (module-level debounce).
//
// All three session-events factories (workspace / active-state / tab-events)
// share ONE 2000ms debounce surface so a multi-bus blip doesn't thrash the
// banner. On fire: stamp flap once, setRecovery(reconnecting), and
// forceSoftHealthProbe() so a healthy host soft-accepts immediately rather
// than waiting up to 25s for the next scheduled tick.
//
// D4b: if any bus reopens healthy before the debounce fires (and recovery
// stayed connected), cancel the debounce and emit soft-resync('events-reopen')
// so half-open grids still repaint without a recovery flip.

import { deriveRecovery } from '@/lib/remote-recovery'
import {
  clearR1FlapEpisode,
  forceSoftHealthProbe,
  stampRemoteReconnectFlap,
} from '@/lib/connection-gate-probe'
import { emitSoftResync } from '@/lib/soft-resync'
import { useConnectHostStore } from '@/stores/connect-host'

/** Debounce window before a session-events drop surfaces recovery (Q1). */
export const REMOTE_EVENTS_DROP_DEBOUNCE_MS = 2000

let debounceTimer: ReturnType<typeof setTimeout> | null = null
/** True after a non-deliberate events close while we care about reopen. */
let hadPriorClose = false

function isRemoteActive(): boolean {
  return useConnectHostStore.getState().activeHost !== 'local'
}

function onDebounceFire(): void {
  debounceTimer = null
  if (!isRemoteActive()) return
  const s = useConnectHostStore.getState()
  // Only surface from the healthy baseline. Wedged / signin-required must
  // not be clobbered; already-reconnecting already owns the banner.
  if (s.recovery.kind !== 'connected') return

  if (import.meta.env.DEV) {
    // eslint-disable-next-line no-console
    console.debug('[recovery] ws-drop debounce fire')
  }

  // D9: one flap stamp per R1 episode (soft-poll fail skips if already set).
  stampRemoteReconnectFlap()
  s.setRecovery(
    deriveRecovery({
      bootStatus: { reachable: false },
      auth: 'unknown',
    }),
  )
  // D2b: never bare setRecovery(reconnecting) — probe immediately so a
  // false blip soft-accepts without waiting for the 25s health cadence.
  forceSoftHealthProbe()
}

/**
 * Session-events non-deliberate close / triggerReconnect path.
 * Starts or resets the module-level debounce when remote + recovery is
 * currently `connected`. Always records hadPriorClose while remote so a
 * later reopen can emit events-reopen soft-resync.
 */
export function noteRemoteEventsClosed(): void {
  if (!isRemoteActive()) return
  hadPriorClose = true
  const kind = useConnectHostStore.getState().recovery.kind
  if (kind !== 'connected') {
    // Already recovering / wedged / sign-in — don't start a new surface.
    return
  }
  if (debounceTimer !== null) clearTimeout(debounceTimer)
  debounceTimer = setTimeout(onDebounceFire, REMOTE_EVENTS_DROP_DEBOUNCE_MS)
}

/**
 * Session-events successful onopen.
 * Cancels a pending R1 debounce. If there was a prior drop and recovery
 * stayed (or is already) connected, emit soft-resync so grids re-attach
 * even when R1 never flipped recovery (half-open hole / D4b).
 */
export function noteRemoteEventsOpened(): void {
  if (debounceTimer !== null) {
    clearTimeout(debounceTimer)
    debounceTimer = null
    if (import.meta.env.DEV) {
      // eslint-disable-next-line no-console
      console.debug('[recovery] ws-drop debounce cancel')
    }
  }
  if (!hadPriorClose) return
  hadPriorClose = false
  if (!isRemoteActive()) return
  if (useConnectHostStore.getState().recovery.kind !== 'connected') return
  emitSoftResync('events-reopen')
}

/**
 * Host switch / deliberate stop — drop debounce and prior-close latch so
 * the next host does not inherit the previous host's drop episode.
 */
export function resetRemoteEventsDropState(): void {
  if (debounceTimer !== null) {
    clearTimeout(debounceTimer)
    debounceTimer = null
  }
  hadPriorClose = false
  clearR1FlapEpisode()
}

/** Test helper: whether a debounce is currently armed. */
export function isRemoteEventsDropDebounceArmed(): boolean {
  return debounceTimer !== null
}

/** Test helper: whether a prior non-deliberate close is latched. */
export function hadRemoteEventsPriorClose(): boolean {
  return hadPriorClose
}

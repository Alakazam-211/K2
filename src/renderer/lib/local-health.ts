// 0.46.18 — the window's LOCAL server, after the app mounted: busy or gone.
//
// Before this, a mounted window on this computer's server had no idea the
// server had stalled. ConnectionGate stops polling a local host once it
// mounts (the remote soft-reconnect machinery is remote-only), so a server
// that took 12 s to answer left every socket reconnecting behind a view that
// looked live but ignored input, with nothing saying why.
//
// Now any sign of trouble on the window's local server (a session-events
// socket closing without us asking, a socket that can't open, a /cli request
// failing at the connection level) starts ONE probe loop of `/boot-status`:
//
//   - single flight: more trouble while the loop runs adds nothing;
//   - each probe waits longer than the last (2 s, 4 s, 8 s, then 15 s), so
//     a busy server gets to answer instead of being asked again every 2 s;
//   - between probes it backs off (0.5 s, 1 s, 2 s, 4 s, 8 s, jittered);
//   - a socket that opens, or a probe that answers, ends it at once.
//
// The state is only ever a light, non-blocking note ("busy" or "can't
// reach"); the app stays mounted and usable on its last view. It is never
// raised for another server's trouble: only the window's own local scope
// reports here (callers check `scope.isWindowHost() && !scope.isRemote`).

import { createStore } from 'zustand/vanilla'
import { jittered } from '@/lib/backoff'
import { PRODUCT_NAME } from '@/lib/brand'

export type LocalHealth =
  | { kind: 'ok' }
  /** The server is there but slow: probes time out or answer with an error. */
  | { kind: 'busy'; since: number }
  /** Nothing listens: the connection is refused. */
  | { kind: 'unreachable'; since: number }

/** What one probe of the local server found. */
export type LocalProbeOutcome = 'ok' | 'slow' | 'refused'

export interface LocalHealthDeps {
  /** One `/boot-status` probe with this timeout. Never throws. */
  probe: (timeoutMs: number) => Promise<LocalProbeOutcome>
  now?: () => number
  sleep?: (ms: number) => Promise<void>
  jitter?: (ms: number) => number
}

/** Per-probe timeouts: a busy server gets longer each time to answer. */
export const LOCAL_PROBE_TIMEOUTS_MS: readonly number[] = [2_000, 4_000, 8_000, 15_000]
/** Waits between failed probes (before jitter). */
export const LOCAL_PROBE_DELAYS_MS: readonly number[] = [500, 1_000, 2_000, 4_000, 8_000]
/** Trouble shorter than this never shows the note (a socket's ordinary
 *  close-and-reopen, a fast restart). */
export const LOCAL_TROUBLE_SURFACE_MS = 1_500

export function localProbeTimeoutMs(attempt: number): number {
  const i = Math.max(0, Math.min(attempt, LOCAL_PROBE_TIMEOUTS_MS.length - 1))
  return LOCAL_PROBE_TIMEOUTS_MS[i]!
}

export function localProbeDelayMs(attempt: number): number {
  const i = Math.max(0, Math.min(attempt, LOCAL_PROBE_DELAYS_MS.length - 1))
  return LOCAL_PROBE_DELAYS_MS[i]!
}

/** The note's copy, or null when there is nothing to say. */
export function localHealthText(h: LocalHealth): string | null {
  if (h.kind === 'busy') return `This computer’s ${PRODUCT_NAME} server is busy. Reconnecting…`
  if (h.kind === 'unreachable') return `Can’t reach this computer’s ${PRODUCT_NAME} server. Reconnecting…`
  return null
}

export const localHealthStore = createStore<{ health: LocalHealth }>(() => ({
  health: { kind: 'ok' },
}))

let deps: LocalHealthDeps | null = null
/** Bumped on every stop / recovery so a loop that is mid-probe drops its
 *  result instead of writing a stale state. */
let generation = 0
let loopRunning = false
let probesInFlight = 0
let loopsStarted = 0

function setHealth(next: LocalHealth): void {
  const cur = localHealthStore.getState().health
  if (cur.kind === next.kind) return
  localHealthStore.setState({ health: next })
}

async function runLoop(gen: number, d: LocalHealthDeps): Promise<void> {
  const now = d.now ?? Date.now
  const sleep = d.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
  const jitter = d.jitter ?? ((ms: number) => jittered(ms))
  const troubleSince = now()
  let attempt = 0
  loopRunning = true
  loopsStarted += 1
  try {
    while (gen === generation) {
      probesInFlight += 1
      let outcome: LocalProbeOutcome
      try {
        outcome = await d.probe(localProbeTimeoutMs(attempt))
      } finally {
        probesInFlight -= 1
      }
      if (gen !== generation) return
      if (outcome === 'ok') {
        setHealth({ kind: 'ok' })
        return
      }
      if (now() - troubleSince >= LOCAL_TROUBLE_SURFACE_MS) {
        setHealth(
          outcome === 'refused'
            ? { kind: 'unreachable', since: troubleSince }
            : { kind: 'busy', since: troubleSince },
        )
      }
      await sleep(jitter(localProbeDelayMs(attempt)))
      attempt += 1
    }
  } finally {
    if (gen === generation) loopRunning = false
  }
}

/**
 * Something on the window's local server failed (a socket dropped or could
 * not open, a request failed at the connection level). Starts the probe
 * loop unless one is already running or no watch is installed.
 */
export function noteLocalTrouble(): void {
  const d = deps
  if (!d || loopRunning) return
  void runLoop(generation, d)
}

/** The window's local server just proved itself (a socket opened). */
export function noteLocalOk(): void {
  if (!loopRunning && localHealthStore.getState().health.kind === 'ok') return
  generation += 1
  loopRunning = false
  setHealth({ kind: 'ok' })
}

/**
 * Install the watch for a mounted local window (ConnectionGate). Returns
 * the stop function; stopping clears the note and ends any loop.
 */
export function startLocalHealthWatch(next: LocalHealthDeps): () => void {
  generation += 1
  loopRunning = false
  deps = next
  setHealth({ kind: 'ok' })
  const mine = next
  return () => {
    if (deps !== mine) return
    deps = null
    generation += 1
    loopRunning = false
    setHealth({ kind: 'ok' })
  }
}

/** Test seams. */
export function localHealthLoopRunningForTests(): boolean {
  return loopRunning
}
export function localHealthProbesInFlightForTests(): number {
  return probesInFlight
}
export function localHealthLoopsStartedForTests(): number {
  return loopsStarted
}
export function resetLocalHealthForTests(): void {
  deps = null
  generation += 1
  loopRunning = false
  probesInFlight = 0
  loopsStarted = 0
  localHealthStore.setState({ health: { kind: 'ok' } })
}

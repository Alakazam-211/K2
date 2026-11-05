// Paused start (prd-zen-user-widgets-v2 UW32 step 2, TUW4.4).
//
// A custom widget that never yields (`while (true) {}`) freezes the window
// it runs in; the per-window watchdog in the shell (`src-tauri/src/lib.rs`)
// reloads that window. Without a memory of what was running, the reload
// would mount the same widget and freeze again. So:
//
//   - Before a Garden mounts any custom widget, the host calls
//     `markZenWidgetsRunning(garden)`: `k2.zen.widgetsRunning.<label>` =
//     `{garden, at, beats: 0}` in this window's localStorage.
//   - Every renderer heartbeat (index.tsx, ~3 s) calls
//     `noteZenWidgetsHeartbeat()`, which counts healthy beats on the marker.
//   - When the Garden's custom widgets unmount (Garden hidden, switched,
//     Zen off), the host calls `clearZenWidgetsRunning()`.
//   - At boot, `takeZenPausedAtBoot()` (index.tsx, before anything mounts)
//     decides once: the widgets start PAUSED when the marker survived from
//     the last run and either the watchdog reloaded this window
//     (`watchdog_take_reload_note`) or the widgets had not yet lived three
//     healthy beats (they froze while starting, even if the person quit
//     before the watchdog acted). A normal quit and relaunch with a Garden
//     open runs its widgets as usual.
//   - The page shows "K2 restarted while this Garden's widgets were
//     running. They're paused. [Run them]" while `zenPausedStart()` names
//     its Garden; **Run them** calls `resumeZenPausedStart()`.
//
// Relaunch first (Rosson 2026-11-05: "If K2 restarts while widgets are
// running ..., relaunch the widgets after the restart is done"). A restart
// that looks like a freeze no longer pauses at once:
//   - a window that was hidden when it went quiet (`hidden` on the marker,
//     kept by `noteZenWidgetsVisibility` on every visibilitychange) runs its
//     widgets: a hidden webview's timers are throttled, so its watchdog
//     reload is not a freeze anyone saw (on z3mbp the watchdog log holds
//     thousands of such reloads);
//   - otherwise K2 relaunches them once on its own (`autoAt` on the next
//     marker), and pauses only when that relaunch is followed by another
//     freeze within `ZEN_WIDGETS_AUTO_RELAUNCH_WINDOW_MS`: a widget stuck in
//     `while (true) {}` costs one more reload, never a loop.
//
// Storage is per window (the label), wrapped in try/catch: a window with no
// storage never pauses (and never loops worse than before this feature).

/** Healthy heartbeats after which a mount counts as having started fine. */
export const ZEN_WIDGETS_HEALTHY_BEATS = 3

/** After K2 relaunched a Garden's widgets on its own, another freeze within
 *  this long pauses them (the freeze is theirs). */
export const ZEN_WIDGETS_AUTO_RELAUNCH_WINDOW_MS = 10 * 60_000

export interface ZenWidgetsRunning {
  garden: string
  /** When the widgets were mounted (unix ms). */
  at: number
  /** Healthy renderer heartbeats since. */
  beats: number
  /** The window was hidden at the last visibility change (its timers are
   *  throttled; a watchdog reload then isn't a freeze). Absent: visible. */
  hidden?: boolean
  /** This mount is K2's automatic relaunch after a freeze (unix ms). */
  autoAt?: number
}

export interface ZenPausedStart {
  garden: string
  /** Why: the watchdog reloaded this window, or the widgets froze while starting. */
  reason: 'watchdog' | 'starting'
}

type StorageLike = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

export function zenWidgetsRunningKey(label: string): string {
  return `k2.zen.widgetsRunning.${label}`
}

function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

function currentLabel(): string {
  try {
    const meta = (globalThis as { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } } })
      .__TAURI_INTERNALS__?.metadata?.currentWindow?.label
    return typeof meta === 'string' && meta ? meta : 'main'
  } catch {
    return 'main'
  }
}

export function readZenWidgetsRunning(label: string, storage: StorageLike | null = defaultStorage()): ZenWidgetsRunning | null {
  if (!storage) return null
  try {
    const raw = storage.getItem(zenWidgetsRunningKey(label))
    if (!raw) return null
    const v = JSON.parse(raw) as Partial<ZenWidgetsRunning>
    if (typeof v.garden !== 'string' || !v.garden || typeof v.at !== 'number' || typeof v.beats !== 'number') return null
    const out: ZenWidgetsRunning = { garden: v.garden, at: v.at, beats: v.beats }
    if (v.hidden === true) out.hidden = true
    if (typeof v.autoAt === 'number') out.autoAt = v.autoAt
    return out
  } catch {
    return null
  }
}

function write(label: string, v: ZenWidgetsRunning | null, storage: StorageLike | null): void {
  if (!storage) return
  try {
    if (v) storage.setItem(zenWidgetsRunningKey(label), JSON.stringify(v))
    else storage.removeItem(zenWidgetsRunningKey(label))
  } catch {
    /* storage full or blocked: the feature degrades to "never paused" */
  }
}

function documentHidden(): boolean {
  try {
    return typeof document !== 'undefined' && document.visibilityState === 'hidden'
  } catch {
    return false
  }
}

/** Before a Garden mounts its custom widgets. A mount that is this boot's
 *  automatic relaunch of the same Garden carries `autoAt`. */
export function markZenWidgetsRunning(
  garden: string,
  opts: { label?: string; now?: number; storage?: StorageLike | null; hidden?: boolean } = {},
): void {
  const storage = opts.storage === undefined ? defaultStorage() : opts.storage
  const m: ZenWidgetsRunning = { garden, at: opts.now ?? Date.now(), beats: 0 }
  if (opts.hidden ?? documentHidden()) m.hidden = true
  if (autoAtBoot && autoAtBoot.garden === garden) m.autoAt = autoAtBoot.at
  write(opts.label ?? currentLabel(), m, storage)
}

/** On every visibilitychange (index.tsx): keep `hidden` on a live marker. */
export function noteZenWidgetsVisibility(
  opts: { label?: string; storage?: StorageLike | null; hidden?: boolean } = {},
): void {
  const storage = opts.storage === undefined ? defaultStorage() : opts.storage
  const label = opts.label ?? currentLabel()
  const m = readZenWidgetsRunning(label, storage)
  if (!m) return
  const hidden = opts.hidden ?? documentHidden()
  if ((m.hidden === true) === hidden) return
  const next: ZenWidgetsRunning = { ...m }
  if (hidden) next.hidden = true
  else delete next.hidden
  write(label, next, storage)
}

/** When the Garden's custom widgets unmount. */
export function clearZenWidgetsRunning(opts: { label?: string; storage?: StorageLike | null } = {}): void {
  const storage = opts.storage === undefined ? defaultStorage() : opts.storage
  write(opts.label ?? currentLabel(), null, storage)
}

/** On every renderer heartbeat: count it on a live marker. A no-op until
 *  the boot decision was taken, so a beat can't make the PREVIOUS run's
 *  marker look healthy. */
export function noteZenWidgetsHeartbeat(opts: { label?: string; storage?: StorageLike | null } = {}): void {
  if (!bootTaken) return
  const storage = opts.storage === undefined ? defaultStorage() : opts.storage
  const label = opts.label ?? currentLabel()
  const m = readZenWidgetsRunning(label, storage)
  if (!m || m.beats >= ZEN_WIDGETS_HEALTHY_BEATS) return
  write(label, { ...m, beats: m.beats + 1 }, storage)
}

/** Did the last run's widgets look frozen: the watchdog reloaded the window
 *  after they mounted, or they never lived three healthy beats? */
export function zenWidgetsFroze(marker: ZenWidgetsRunning | null, watchdogReloadedAt: number | null): ZenPausedStart | null {
  if (!marker) return null
  if (watchdogReloadedAt !== null && watchdogReloadedAt >= marker.at) return { garden: marker.garden, reason: 'watchdog' }
  if (marker.beats < ZEN_WIDGETS_HEALTHY_BEATS) return { garden: marker.garden, reason: 'starting' }
  return null
}

/** The boot decision (pure): paused, or null to run. A freeze pauses only
 *  when the window was visible and K2 already relaunched these widgets on
 *  its own within `ZEN_WIDGETS_AUTO_RELAUNCH_WINDOW_MS`; otherwise they run
 *  (the first time: K2's automatic relaunch, `zenAutoRelaunch`). */
export function zenPausedStartDecision(
  marker: ZenWidgetsRunning | null,
  watchdogReloadedAt: number | null,
  now: number = Date.now(),
): ZenPausedStart | null {
  const froze = zenWidgetsFroze(marker, watchdogReloadedAt)
  if (!froze || !marker || marker.hidden) return null
  if (marker.autoAt !== undefined && now - marker.autoAt < ZEN_WIDGETS_AUTO_RELAUNCH_WINDOW_MS) return froze
  return null
}

/** Does this boot relaunch a frozen-looking Garden on its own (visible,
 *  not paused)? Then its next marker carries `autoAt`. */
function zenAutoRelaunch(marker: ZenWidgetsRunning | null, watchdogReloadedAt: number | null, now: number): { garden: string; at: number } | null {
  if (!marker || marker.hidden || !zenWidgetsFroze(marker, watchdogReloadedAt)) return null
  if (zenPausedStartDecision(marker, watchdogReloadedAt, now)) return null
  return { garden: marker.garden, at: now }
}

let pausedAtBoot: ZenPausedStart | null = null
let autoAtBoot: { garden: string; at: number } | null = null
let bootTaken = false
let resolveReady: () => void = () => {}
const ready = new Promise<void>((r) => {
  resolveReady = r
})
const listeners = new Set<() => void>()

/** Resolves once this window's boot decision is taken: a Garden awaits it
 *  before mounting custom widgets. */
export function zenPausedStartReady(): Promise<void> {
  return ready
}

/**
 * Once per window boot (index.tsx), before any Garden mounts: decide
 * whether this window's custom widgets start paused, and clear the old
 * marker (the page writes a new one when it mounts widgets again).
 * `reloadNote` is the shell's `watchdog_take_reload_note` answer.
 */
export function takeZenPausedAtBoot(
  reloadNote: number | null,
  opts: { label?: string; storage?: StorageLike | null; now?: number } = {},
): ZenPausedStart | null {
  const storage = opts.storage === undefined ? defaultStorage() : opts.storage
  const label = opts.label ?? currentLabel()
  const now = opts.now ?? Date.now()
  const marker = readZenWidgetsRunning(label, storage)
  pausedAtBoot = zenPausedStartDecision(marker, reloadNote, now)
  autoAtBoot = zenAutoRelaunch(marker, reloadNote, now)
  if (autoAtBoot) console.info(`[zen] K2 restarted while ${autoAtBoot.garden}'s widgets were running: relaunching them`)
  write(label, null, storage)
  bootTaken = true
  resolveReady()
  for (const l of listeners) l()
  return pausedAtBoot
}

/** index.tsx: ask the shell for this window's watchdog note (null outside
 *  the desktop app), then take the boot decision. Never rejects. */
export async function bootZenPausedStart(takeReloadNote: () => Promise<number | null | undefined>): Promise<ZenPausedStart | null> {
  let note: number | null = null
  try {
    const n = await takeReloadNote()
    note = typeof n === 'number' ? n : null
  } catch {
    note = null
  }
  return takeZenPausedAtBoot(note)
}

/** Tests only: forget the boot decision. */
export function resetZenPausedStartForTests(): void {
  pausedAtBoot = null
  autoAtBoot = null
  bootTaken = false
  listeners.clear()
}

/** The Garden whose custom widgets are paused in this window, or null. */
export function zenPausedStart(): ZenPausedStart | null {
  return pausedAtBoot
}

/** **Run them**: lift the pause (the page then mounts and marks again). */
export function resumeZenPausedStart(): void {
  if (!pausedAtBoot) return
  pausedAtBoot = null
  for (const l of listeners) l()
}

/** For a `useSyncExternalStore` over `zenPausedStart`. */
export function subscribeZenPausedStart(cb: () => void): () => void {
  listeners.add(cb)
  return () => listeners.delete(cb)
}

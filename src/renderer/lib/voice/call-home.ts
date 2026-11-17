// One call, one home window: the call's stage (its tiles, its video, the
// camera's self-view) is drawn in exactly one K2 window, its home. Every
// other window shows the slim in-call bar. The home is where the call was
// started or answered (the window focused last), the call's own window while
// it's open (Pop out), or wherever someone pressed Show call here.
//
// The owner token is per client, shared by this app's windows through
// localStorage (the same origin, so a write in one window is a `storage`
// event in the others): the call it's for, the owner window's label and a
// lease the owner renews while it's open. An owner the app still lists as
// open keeps it however late it renews (a background window's timers run
// slowly); an owner that closed keeps it until the lease runs out, then the
// window focused last takes it. Pure, plus the storage. View state only:
// the call itself is the app's engine's, and none of this is daemon state.

/** The owner token. */
export const CALL_HOME_KEY = 'k2.voice.callHome.v1'
/** When each window last had focus (label → ms). */
export const CALL_FOCUS_KEY = 'k2.voice.callFocus.v1'
/** How long an owner that stopped renewing (it closed) keeps the stage. */
export const CALL_HOME_LEASE_MS = 6_000
/** How often each window looks again (and the owner renews). */
export const CALL_HOME_TICK_MS = 1_500

export interface CallHome {
  /** The call's place key (`host|room`). */
  call: string
  /** The owner window's label. */
  owner: string
  /** The lease: the owner holds it at least until then (ms). */
  until: number
}

function homeOf(v: unknown): CallHome | null {
  if (!v || typeof v !== 'object') return null
  const r = v as Record<string, unknown>
  if (typeof r.call !== 'string' || typeof r.owner !== 'string' || !r.owner || typeof r.until !== 'number' || !Number.isFinite(r.until)) return null
  return { call: r.call, owner: r.owner, until: r.until }
}

export function readCallHome(): CallHome | null {
  try {
    const raw = localStorage.getItem(CALL_HOME_KEY)
    return raw ? homeOf(JSON.parse(raw)) : null
  } catch {
    return null
  }
}

export function writeCallHome(h: CallHome): void {
  try {
    localStorage.setItem(CALL_HOME_KEY, JSON.stringify(h))
  } catch {
    // Storage unavailable: each window decides alone (one window works).
  }
}

/** The call ended: its token goes (another call's stays). */
export function clearCallHome(call: string): void {
  try {
    if (readCallHome()?.call === call) localStorage.removeItem(CALL_HOME_KEY)
  } catch {
    // Storage unavailable: nothing kept.
  }
}

export function readWindowFocus(): Record<string, number> {
  try {
    const raw = localStorage.getItem(CALL_FOCUS_KEY)
    const v = raw ? (JSON.parse(raw) as unknown) : null
    if (!v || typeof v !== 'object' || Array.isArray(v)) return {}
    const out: Record<string, number> = {}
    for (const [k, t] of Object.entries(v as Record<string, unknown>)) if (typeof t === 'number' && Number.isFinite(t)) out[k] = t
    return out
  } catch {
    return {}
  }
}

function writeWindowFocus(f: Record<string, number>): void {
  try {
    localStorage.setItem(CALL_FOCUS_KEY, JSON.stringify(f))
  } catch {
    // Storage unavailable: the election falls back to window names.
  }
}

export function noteWindowFocus(label: string, now: number): void {
  writeWindowFocus({ ...readWindowFocus(), [label]: now })
}

/** A window that goes is never elected again. */
export function forgetWindow(label: string): void {
  const f = readWindowFocus()
  if (!(label in f)) return
  delete f[label]
  writeWindowFocus(f)
}

export interface HomeInput {
  /** The call's place key. */
  call: string
  home: CallHome | null
  now: number
  /** The app's open windows (shown or minimised); null where the app can't
   *  list them (then the focus list stands in). */
  live: readonly string[] | null
  focus: Record<string, number>
  /** Taken first when open (the window the call's own window came from). */
  prefer: string | null
  /** Owners known to have closed just now (the call's own window, by the
   *  app's event): their lease counts for nothing. */
  closed?: readonly string[]
  /** Never elected (the call's own window: it holds the stage only when
   *  the stage is popped out to it). */
  never: readonly string[]
}

/** The window that holds the call's stage now, or null when no window is
 *  left. Every window works it out the same way from the same inputs. */
export function decideHome({ call, home, now, live, focus, prefer, never, closed = [] }: HomeInput): string | null {
  const held = home !== null && home.call === call && !closed.includes(home.owner)
  if (held && (home.until > now || (live?.includes(home.owner) ?? false))) return home.owner
  const pool = (live ?? Object.keys(focus)).filter((l) => !never.includes(l))
  if (prefer && pool.includes(prefer)) return prefer
  const sorted = [...pool].sort((a, b) => (focus[b] ?? 0) - (focus[a] ?? 0) || (a < b ? -1 : a > b ? 1 : 0))
  return sorted[0] ?? null
}

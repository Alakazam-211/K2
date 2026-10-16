/** Subscription windows the daemon cached. No provider tokens. */

import { asArray } from '@/lib/as-array'

export interface UsageWindow {
  label: string
  /** 0–1 used. Percent left is `1 - used`. */
  used: number
  resetsAt: string
  /** Usage v2 (served by newer daemons; absent on older ones). Stable id:
   *  `five_hour`, `seven_day`, `seven_day_oauth_apps`, `model:<name>:<kind>`,
   *  `primary`, `secondary`, `credit`, `grok_build`. */
  id?: string
  /** `session` · `weekly` · `monthly` · `daily` · `other`. */
  kind?: string
  /** 0..100, the daemon's own rounding of `used`. */
  usedPercent?: number
  /** The window's length; absent = no pace mark. */
  windowSecs?: number
  /** `resetsAt − windowSecs` (RFC3339). */
  windowStart?: string
  /** `resetsAt` had passed when the daemon served it; a re-check is under way. */
  resetPassed?: boolean
}

/** Why a login has (no) numbers (Usage v2 §3.2). */
export type LimitState =
  | 'ok'
  | 'no_window'
  | 'not_signed_in'
  | 'expired'
  | 'probe_failed'
  | 'unsupported'
  | 'airgap'

export interface HarnessUsage {
  harness: string
  plan: string
  windows: UsageWindow[]
  checkedAt: string
  /** "" when signed in with windows. Otherwise an empty-state sentence. */
  status: string
  /** The last check failed (why); the windows are the last good reading
   *  and `checkedAt` is when that was. Newer daemons only. */
  error?: string
  /** Usage v2: typed reason (filled when served). Derive with `limitState`. */
  state?: LimitState | string
  /** `probe` · `live_statusline` · `live_rollout`. */
  source?: string
  /** The daemon is backing off this check until then (RFC3339). */
  nextProbeAt?: string
  /** When the failed check ran. */
  attemptedAt?: string
}

export interface SubscriptionDoc {
  harnesses: HarnessUsage[]
  /** The daemon is in air-gap mode: it does not check usage at all. */
  airgap?: true
}

/** True when the daemon said it skips the usage check for air-gap. */
export function isAirgapped(doc: SubscriptionDoc | null | undefined): boolean {
  return doc?.airgap === true
}

/** Clock-menu probes. Anything else is absent — do not invent a row. */
export const PROBED_HARNESSES = ['claude', 'codex', 'grok'] as const

const STALE_MS = 15_000
const PROBED = new Set<string>(PROBED_HARNESSES)

function isRecord(raw: unknown): raw is Record<string, unknown> {
  return raw !== null && typeof raw === 'object' && !Array.isArray(raw)
}

function str(raw: unknown): string {
  return typeof raw === 'string' ? raw : ''
}

function finite(raw: unknown): number | undefined {
  return typeof raw === 'number' && Number.isFinite(raw) ? raw : undefined
}

function nonEmpty(raw: unknown): string | undefined {
  return typeof raw === 'string' && raw !== '' ? raw : undefined
}

export function parseWindow(raw: unknown): UsageWindow | null {
  if (!isRecord(raw)) return null
  if (typeof raw.label !== 'string') return null
  if (typeof raw.used !== 'number' || !Number.isFinite(raw.used)) return null
  const w: UsageWindow = { label: raw.label, used: raw.used, resetsAt: str(raw.resetsAt) }
  const id = nonEmpty(raw.id)
  if (id) w.id = id
  const kind = nonEmpty(raw.kind)
  if (kind) w.kind = kind
  const pct = finite(raw.usedPercent)
  if (pct !== undefined) w.usedPercent = pct
  const secs = finite(raw.windowSecs)
  if (secs !== undefined && secs > 0) w.windowSecs = secs
  const start = nonEmpty(raw.windowStart)
  if (start) w.windowStart = start
  if (raw.resetPassed === true) w.resetPassed = true
  return w
}

/** The v2 row fields (`state`, `source`, `nextProbeAt`, `attemptedAt`). */
export function parseRowExtras(raw: Record<string, unknown>): Partial<HarnessUsage> {
  const out: Partial<HarnessUsage> = {}
  const state = nonEmpty(raw.state)
  if (state) out.state = state
  const source = nonEmpty(raw.source)
  if (source) out.source = source
  const next = nonEmpty(raw.nextProbeAt)
  if (next) out.nextProbeAt = next
  const attempted = nonEmpty(raw.attemptedAt)
  if (attempted) out.attemptedAt = attempted
  return out
}

function parseHarness(raw: unknown): HarnessUsage | null {
  if (!isRecord(raw)) return null
  if (typeof raw.harness !== 'string' || raw.harness === '') return null
  const windows: UsageWindow[] = []
  for (const w of asArray(raw.windows)) {
    const window = parseWindow(w)
    if (window) windows.push(window)
  }
  return {
    harness: raw.harness,
    plan: str(raw.plan),
    windows,
    checkedAt: str(raw.checkedAt),
    status: str(raw.status),
    ...(typeof raw.error === 'string' && raw.error !== '' ? { error: raw.error } : {}),
    ...parseRowExtras(raw),
  }
}

/**
 * The `usage/subscriptions` body as a real `SubscriptionDoc`. Anything that
 * is not the daemon's shape (an error envelope, `{}`, `null`, a string, or
 * another route's body) becomes `{ harnesses: [] }`; rows and windows that
 * are not well-formed are dropped. Use it wherever a body enters state.
 */
export function parseSubscriptionDoc(raw: unknown): SubscriptionDoc {
  if (!isRecord(raw)) return { harnesses: [] }
  const harnesses: HarnessUsage[] = []
  for (const h of asArray(raw.harnesses)) {
    const row = parseHarness(h)
    if (row) harnesses.push(row)
  }
  return raw.airgap === true ? { harnesses, airgap: true } : { harnesses }
}

export function visibleHarnesses(doc: SubscriptionDoc | null | undefined): HarnessUsage[] {
  if (!doc || !Array.isArray(doc.harnesses)) return []
  return doc.harnesses.filter((h) => PROBED.has(h.harness))
}

export function harnessName(id: string): string {
  if (id === 'claude') return 'Claude'
  if (id === 'codex') return 'Codex'
  if (id === 'grok') return 'Grok'
  return id
}

/** A live window is a signed-in row that has a plan meter or an explicit empty state.
 * A blank status with no windows is a failed probe, not a signed-in account. */
export function isSignedIn(row: HarnessUsage): boolean {
  if (row.status === 'Not signed in' || row.status === 'Sign-in expired') return false
  if ((row.windows?.length ?? 0) === 0 && row.status === '') return false
  return true
}

/** Claude `utilization`, Codex `usedPercent`, and Grok `creditUsagePercent`, as a whole percent used. */
export function percentUsed(used: number): number {
  const clamped = Math.min(1, Math.max(0, used))
  return Math.round(clamped * 100)
}

type ProbedHarness = (typeof PROBED_HARNESSES)[number]

function isProbedHarness(id: string): id is ProbedHarness {
  return (PROBED_HARNESSES as readonly string[]).includes(id)
}

/** Claude's chip is the greater of its Session (5-hour) and Weekly windows. Model-scoped limits such as "Fable Weekly" stay in the menu but never drive the chip. Other harnesses use every window. */
const CLAUDE_CHIP_LABELS = new Set(['Session', 'Weekly'])

function chipWindows<W extends { label: string }>(harness: ProbedHarness, windows: W[]): W[] {
  if (harness !== 'claude') return windows
  return windows.filter((window) => CLAUDE_CHIP_LABELS.has(window.label))
}

/** One top-bar chip per signed-in harness that has a window. Highest window wins inside that harness. Claude, then Codex, then Grok.
 *  With `nowMs`, a window past its reset counts 0 (Usage v2 P8: the old % is not shown as current). */
export function buttonChips(
  doc: SubscriptionDoc | null,
  nowMs?: number,
): { harness: ProbedHarness; used: number }[] {
  if (!doc || isAirgapped(doc)) return []
  const chips: { harness: ProbedHarness; used: number }[] = []
  for (const row of visibleHarnesses(doc)) {
    if (!isProbedHarness(row.harness)) continue
    if (!isSignedIn(row) || !Array.isArray(row.windows) || row.windows.length === 0) continue
    let used = 0
    for (const window of chipWindows(row.harness, row.windows)) {
      used = Math.max(used, nowMs === undefined ? percentUsed(window.used) : windowPercent(window, nowMs))
    }
    chips.push({ harness: row.harness, used })
  }
  const order = new Map(PROBED_HARNESSES.map((id, index) => [id, index]))
  chips.sort((a, b) => (order.get(a.harness) ?? 99) - (order.get(b.harness) ?? 99))
  return chips
}

/** Highest used percent among signed-in windows. Null when nobody is signed in. */
export function buttonSummary(
  doc: SubscriptionDoc | null,
): { harness: string; used: number } | null {
  const chips = buttonChips(doc)
  let best: { harness: string; used: number } | null = null
  for (const chip of chips) {
    if (best === null || chip.used > best.used) best = chip
  }
  return best
}

/**
 * Every signed-in harness that has a window, highest window each.
 * No signed-in window → "Usage".
 */
export function buttonLabel(doc: SubscriptionDoc | null): string {
  const chips = buttonChips(doc)
  if (chips.length === 0) return 'Usage'
  return chips.map((chip) => `${harnessName(chip.harness)} ${chip.used}%`).join(' ')
}

/** True when the menu should POST a refresh instead of only reading. */
export function isStale(doc: SubscriptionDoc | null, now: number): boolean {
  const rows = visibleHarnesses(doc)
  if (rows.length === 0) return true
  for (const name of PROBED_HARNESSES) {
    const row = rows.find((h) => h.harness === name)
    if (!row) return true
    const checked = Date.parse(row.checkedAt)
    if (Number.isNaN(checked)) return true
    if (now - checked > STALE_MS) return true
  }
  return false
}

/** Time until `resetsAt`: `3h`, `4d`, `45m`. */
export function formatResetsIn(resetsAt: string, nowMs: number): string {
  const t = Date.parse(resetsAt)
  if (Number.isNaN(t)) return ''
  const delta = t - nowMs
  if (delta <= 0) return 'now'
  const minutes = Math.round(delta / 60_000)
  if (minutes < 90) return `${Math.max(1, minutes)}m`
  const hours = Math.round(delta / 3_600_000)
  if (hours < 48) return `${hours}h`
  const days = Math.round(delta / 86_400_000)
  return `${days}d`
}

// ── Usage v2 limit bars (P8, Q4: T3-style bars with a pace mark) ─────────
//
// The daemon owns the numbers, the state and the freshness; the renderer
// only draws them. The pace mark is the one figure drawn here: where even
// spending would be, from `windowStart` / `windowSecs` and the clock.

/** Amber from here (Rosson 2026-10-16). */
export const LIMIT_WARN_PCT = 75
/** Red from here. */
export const LIMIT_OVER_PCT = 90
/** T3: within ±5 points of the pace mark reads "on pace". */
export const PACE_BAND_PCT = 5

/** The window's reset time has passed: the daemon said so when it served
 *  it, or the clock has moved past `resetsAt` since. */
export function windowReset(w: UsageWindow, nowMs: number): boolean {
  if (w.resetPassed === true) return true
  const t = Date.parse(w.resetsAt)
  return !Number.isNaN(t) && t <= nowMs
}

function rawPercent(w: UsageWindow): number {
  const raw = typeof w.usedPercent === 'number' ? w.usedPercent : w.used * 100
  return Math.min(100, Math.max(0, raw))
}

/** Whole percent used to draw. A window past its reset draws 0 until the
 *  re-check lands ("reset — re-checking"). */
export function windowPercent(w: UsageWindow, nowMs: number): number {
  if (windowReset(w, nowMs)) return 0
  return Math.round(rawPercent(w))
}

export type LimitTone = 'ok' | 'warn' | 'over'

/** Colour band: amber at 75%, red at 90%. */
export function limitTone(percent: number): LimitTone {
  if (percent >= LIMIT_OVER_PCT) return 'over'
  if (percent >= LIMIT_WARN_PCT) return 'warn'
  return 'ok'
}

/** The bar fill for a tone (the label says the same in words). */
export function limitToneColor(tone: LimitTone): string {
  if (tone === 'over') return '#ef4444'
  if (tone === 'warn') return '#f59e0b'
  return 'var(--color-accent)'
}

export interface PaceMark {
  /** Where even spending would be now, 0..100. */
  elapsedPct: number
  /** Used minus elapsed, in points. */
  delta: number
  /** `ahead`: spending faster than even (may run out before reset). */
  status: 'ahead' | 'on' | 'under'
}

/** The pace mark for a window, or null when the daemon gave no length, the
 *  start is unknown, or the window has reset. */
export function paceMark(w: UsageWindow, nowMs: number): PaceMark | null {
  if (!w.windowSecs || w.windowSecs <= 0) return null
  if (windowReset(w, nowMs)) return null
  let start = w.windowStart ? Date.parse(w.windowStart) : NaN
  if (Number.isNaN(start)) {
    const reset = Date.parse(w.resetsAt)
    if (Number.isNaN(reset)) return null
    start = reset - w.windowSecs * 1000
  }
  const elapsed = Math.min(1, Math.max(0, (nowMs - start) / (w.windowSecs * 1000))) * 100
  const delta = rawPercent(w) - elapsed
  const status = delta > PACE_BAND_PCT ? 'ahead' : delta < -PACE_BAND_PCT ? 'under' : 'on'
  return { elapsedPct: Math.round(elapsed * 10) / 10, delta: Math.round(delta * 10) / 10, status }
}

export function paceText(p: PaceMark): string {
  if (p.status === 'ahead') return 'ahead of pace'
  if (p.status === 'under') return 'under pace'
  return 'on pace'
}

const LIMIT_STATES: readonly string[] = ['ok', 'no_window', 'not_signed_in', 'expired', 'probe_failed', 'unsupported', 'airgap']

type StateInput = Pick<HarnessUsage, 'state' | 'status' | 'error' | 'windows'>

/** The row's typed state: the daemon's when it sent one, else derived the
 *  daemon's way (`subscription_usage::derive_state`) for an older daemon or
 *  a raw wallet reading. */
export function limitState(row: StateInput): LimitState {
  if (typeof row.state === 'string' && LIMIT_STATES.includes(row.state)) return row.state as LimitState
  switch (row.status) {
    case 'Off in air-gap mode':
      return 'airgap'
    case 'Not signed in':
      return 'not_signed_in'
    case 'Sign-in expired':
      return 'expired'
    case 'Not supported':
      return 'unsupported'
  }
  if (row.error) return 'probe_failed'
  if ((row.windows?.length ?? 0) === 0) return 'no_window'
  return 'ok'
}

/** Why there are no (fresh) numbers, in words; null when the row is ok. */
export function limitStateText(row: StateInput): string | null {
  switch (limitState(row)) {
    case 'ok':
      return null
    case 'not_signed_in':
      return 'Not signed in'
    case 'expired':
      return 'Sign-in expired. Sign in again to see its limits.'
    case 'probe_failed':
      return row.error ? `Last check failed (${row.error})` : 'Last check failed'
    case 'unsupported':
      return 'No limits for this login (API token or not supported)'
    case 'no_window':
      return row.status ? row.status : 'No usage window'
    case 'airgap':
      return 'Off in air-gap mode'
  }
}

/** The row needs a person: its last check failed or its sign-in expired. */
export function needsAttention(row: StateInput): boolean {
  const state = limitState(row)
  return state === 'probe_failed' || state === 'expired' || Boolean(row.error)
}

/** "updated 4m ago" from an RFC3339 time; "not checked yet" when empty. */
export function updatedAgo(checkedAt: string | null | undefined, nowMs: number): string {
  const t = checkedAt ? Date.parse(checkedAt) : NaN
  if (Number.isNaN(t)) return 'not checked yet'
  const s = Math.max(0, Math.floor((nowMs - t) / 1000))
  if (s < 60) return 'updated just now'
  const m = Math.floor(s / 60)
  if (m < 60) return `updated ${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 48) return `updated ${h}h ago`
  return `updated ${Math.floor(h / 24)}d ago`
}

/** Where a reading came from, for the freshness line; null for a probe. */
export function sourceText(source: string | undefined): string | null {
  if (source === 'live_statusline' || source === 'live_rollout') return 'live from a running session'
  return null
}

/** Rows the top-bar menu lists (P8, B22): every probed harness that is
 *  signed in, including one whose sign-in expired or whose check failed
 *  with no windows left (they say why and offer an action). Rows that are
 *  plainly not signed in, not supported or air-gapped stay out. */
export function menuRows(doc: SubscriptionDoc | null | undefined): HarnessUsage[] {
  return visibleHarnesses(doc).filter((row) => {
    const state = limitState(row)
    if (state === 'not_signed_in' || state === 'unsupported' || state === 'airgap') return false
    if (state === 'expired' || state === 'probe_failed') return true
    return isSignedIn(row)
  })
}

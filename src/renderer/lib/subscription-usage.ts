/** Subscription windows the daemon cached. No provider tokens. */

import { asArray } from '@/lib/as-array'

export interface UsageWindow {
  label: string
  /** 0–1 used. Percent left is `1 - used`. */
  used: number
  resetsAt: string
}

export interface HarnessUsage {
  harness: string
  plan: string
  windows: UsageWindow[]
  checkedAt: string
  /** "" when signed in with windows. Otherwise an empty-state sentence. */
  status: string
}

export interface SubscriptionDoc {
  harnesses: HarnessUsage[]
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

function parseWindow(raw: unknown): UsageWindow | null {
  if (!isRecord(raw)) return null
  if (typeof raw.label !== 'string') return null
  if (typeof raw.used !== 'number' || !Number.isFinite(raw.used)) return null
  return { label: raw.label, used: raw.used, resetsAt: str(raw.resetsAt) }
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
  return { harnesses }
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

/** One top-bar chip per signed-in harness that has a window. Highest window wins inside that harness. Claude, then Codex, then Grok. */
export function buttonChips(
  doc: SubscriptionDoc | null,
): { harness: ProbedHarness; used: number }[] {
  if (!doc) return []
  const chips: { harness: ProbedHarness; used: number }[] = []
  for (const row of visibleHarnesses(doc)) {
    if (!isProbedHarness(row.harness)) continue
    if (!isSignedIn(row) || !Array.isArray(row.windows) || row.windows.length === 0) continue
    let used = 0
    for (const window of chipWindows(row.harness, row.windows)) {
      used = Math.max(used, percentUsed(window.used))
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

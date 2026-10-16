// Usage v2 §3.3 — `GET /cli/usage/limits`: every login of every tool with
// its subscription windows, default first, then pinned, then idle. Owner
// and Admin only (a Member gets 403). The daemon owns the numbers, the
// typed state and the freshness; this file only parses and summarises.

import { asArray } from '@/lib/as-array'
import {
  harnessName,
  limitState,
  parseRowExtras,
  parseWindow,
  windowPercent,
  windowReset,
  formatResetsIn,
  type HarnessUsage,
  type UsageWindow,
} from '@/lib/subscription-usage'

export interface LimitsPin {
  scopeKind: string
  scopeId: string
  label?: string
}

/** One login's limits. The `HarnessUsage` fields are the same reading the
 *  top bar uses (state, windows, checkedAt, error, source, nextProbeAt). */
export interface LimitsLogin extends HarnessUsage {
  /** Wallet id; null for a CLI signed in with no wallet entry ("unmanaged"). */
  accountId: string | null
  label: string
  email: string | null
  /** `subscription` · `api_key` · `unmanaged`. */
  kind: string
  isDefault: boolean
  isLive: boolean
  pinned: LimitsPin[]
}

export interface LimitsTool {
  tool: string
  display: string
  supported: boolean
  logins: LimitsLogin[]
}

export interface LimitsDoc {
  generatedAt: string
  airgap: boolean
  tools: LimitsTool[]
}

type Rec = Record<string, unknown>

function isRecord(raw: unknown): raw is Rec {
  return raw !== null && typeof raw === 'object' && !Array.isArray(raw)
}

function str(raw: unknown): string {
  return typeof raw === 'string' ? raw : ''
}

function parsePin(raw: unknown): LimitsPin | null {
  if (!isRecord(raw)) return null
  const pin: LimitsPin = { scopeKind: str(raw.scopeKind), scopeId: str(raw.scopeId) }
  if (typeof raw.label === 'string' && raw.label !== '') pin.label = raw.label
  return pin
}

function parseLogin(raw: unknown, tool: string): LimitsLogin | null {
  if (!isRecord(raw)) return null
  const windows: UsageWindow[] = []
  for (const w of asArray(raw.windows)) {
    const window = parseWindow(w)
    if (window) windows.push(window)
  }
  const pinned: LimitsPin[] = []
  for (const p of asArray(raw.pinned)) {
    const pin = parsePin(p)
    if (pin) pinned.push(pin)
  }
  return {
    harness: tool,
    plan: str(raw.plan),
    windows,
    checkedAt: str(raw.checkedAt),
    status: str(raw.status),
    ...(typeof raw.error === 'string' && raw.error !== '' ? { error: raw.error } : {}),
    ...parseRowExtras(raw),
    accountId: typeof raw.accountId === 'string' && raw.accountId !== '' ? raw.accountId : null,
    label: str(raw.label),
    email: typeof raw.email === 'string' && raw.email !== '' ? raw.email : null,
    kind: str(raw.kind),
    isDefault: raw.isDefault === true,
    isLive: raw.isLive === true,
    pinned,
  }
}

/** The route's body as a real doc; anything else is an empty doc. */
export function parseLimitsDoc(raw: unknown): LimitsDoc {
  if (!isRecord(raw)) return { generatedAt: '', airgap: false, tools: [] }
  const tools: LimitsTool[] = []
  for (const t of asArray(raw.tools)) {
    if (!isRecord(t) || typeof t.tool !== 'string' || t.tool === '') continue
    const logins: LimitsLogin[] = []
    for (const l of asArray(t.logins)) {
      const login = parseLogin(l, t.tool)
      if (login) logins.push(login)
    }
    tools.push({
      tool: t.tool,
      display: typeof t.display === 'string' && t.display !== '' ? t.display : harnessName(t.tool),
      supported: t.supported === true,
      logins,
    })
  }
  return { generatedAt: str(raw.generatedAt), airgap: raw.airgap === true, tools }
}

/** The login with this wallet id, anywhere in the doc. */
export function findLimitsLogin(doc: LimitsDoc | null | undefined, accountId: string): LimitsLogin | null {
  if (!doc) return null
  for (const tool of doc.tools) {
    for (const login of tool.logins) {
      if (login.accountId === accountId) return login
    }
  }
  return null
}

/** One entry of the Usage page's slim strip: a tool's default login. */
export interface LimitsStripItem {
  tool: string
  display: string
  /** "62%" of the highest session/weekly window, or null with `note`. */
  percent: number | null
  /** "resets 3h", "reset — re-checking", or the reason there is no number. */
  note: string
  warn: boolean
}

/** The windows a strip/chip summary uses: Claude's model-scoped weeks stay
 *  on the Subs / Tokens rows (as on the top-bar chip). */
function summaryWindows(tool: string, windows: UsageWindow[]): UsageWindow[] {
  if (tool !== 'claude') return windows
  const main = windows.filter((w) => w.id === 'five_hour' || w.id === 'seven_day' || w.label === 'Session' || w.label === 'Weekly')
  return main.length > 0 ? main : windows
}

/** "Claude 62% · resets 3h · Codex 18% …": each supported tool's default
 *  login (the first one listed), highest window. Tools with no login are left
 *  out; a login with no numbers says why in a word or two. */
export function limitsStrip(doc: LimitsDoc | null | undefined, nowMs: number): LimitsStripItem[] {
  if (!doc || doc.airgap) return []
  const out: LimitsStripItem[] = []
  for (const tool of doc.tools) {
    if (!tool.supported) continue
    const login = tool.logins.find((l) => l.isDefault) ?? tool.logins[0]
    if (!login) continue
    const state = limitState(login)
    if (state === 'not_signed_in' || state === 'unsupported' || state === 'airgap') continue
    const windows = summaryWindows(tool.tool, login.windows)
    if (windows.length === 0) {
      out.push({
        tool: tool.tool,
        display: tool.display,
        percent: null,
        note: state === 'expired' ? 'sign-in expired' : state === 'probe_failed' ? 'check failed' : 'no window',
        warn: state === 'expired' || state === 'probe_failed',
      })
      continue
    }
    let top: UsageWindow = windows[0]
    for (const w of windows) {
      if (windowPercent(w, nowMs) > windowPercent(top, nowMs)) top = w
    }
    const reset = windowReset(top, nowMs)
    const until = formatResetsIn(top.resetsAt, nowMs)
    out.push({
      tool: tool.tool,
      display: tool.display,
      percent: windowPercent(top, nowMs),
      note: reset ? 'reset — re-checking' : until ? `resets ${until}` : '',
      warn: state === 'probe_failed' || Boolean(login.error),
    })
  }
  return out
}

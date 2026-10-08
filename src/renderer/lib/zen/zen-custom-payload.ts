// prd-zen-user-widgets-v2 UW38 (+ UWB4, UWB7, UWB9, UWB13) — a custom
// widget as `GET /cli/zen/get` sends it, parsed at the boundary.
//
// The daemon resolves each `[[widget]] kind = "custom"` placement to
// `ZenCustomWidgetPayload` (`zen-custom-types.ts`): the manifest's name,
// description, reasons and requested caps, the bundle hash and state, and
// the grant the daemon verified (signature, key, widget and ask). Nothing
// past this file sees raw JSON. A field this client can't read falls back
// to the closed, safe value: no caps, no grant, `broken`.

import { USER_WIDGET_CAPS, type UserWidgetCap } from '../k2-caps.generated'
import type {
  ZenCustomWidgetPayload,
  ZenGrantEntry,
  ZenGrantPause,
  ZenGrantState,
  ZenGrantView,
  ZenScope,
  ZenWidgetFinding,
} from './zen-custom-types'
import type { ZenLibRef } from './zen-lib-loader'

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function str(v: unknown): string | null {
  return typeof v === 'string' ? v : null
}

const WIDGET_CAPS: ReadonlySet<string> = new Set(USER_WIDGET_CAPS)

/** Only the four widget caps survive, deduplicated, in the order given. */
export function zenWidgetCaps(raw: unknown): UserWidgetCap[] {
  if (!Array.isArray(raw)) return []
  const out: UserWidgetCap[] = []
  for (const c of raw) {
    if (typeof c === 'string' && WIDGET_CAPS.has(c) && !out.includes(c as UserWidgetCap)) out.push(c as UserWidgetCap)
  }
  return out
}

const GRANT_STATES: readonly ZenGrantState[] = ['none', 'invalid', 'review', 'partial', 'granted']

/** A scope from its one-key wire form (`{home}`, `{allHomes: true}`, …), or null. */
export function parseZenScope(raw: unknown): ZenScope | null {
  if (!isObj(raw)) return null
  const keys = Object.keys(raw)
  if (keys.length !== 1) return null
  const [k] = keys
  const v = raw[k]
  switch (k) {
    case 'agent':
    case 'home':
    case 'server':
      return typeof v === 'string' && v.trim() ? ({ [k]: v } as ZenScope) : null
    case 'homes': {
      if (!Array.isArray(v) || v.length === 0) return null
      const ids = v.filter((x): x is string => typeof x === 'string' && x.trim().length > 0)
      if (ids.length !== v.length || new Set(ids).size !== ids.length) return null
      return { homes: ids }
    }
    case 'allHomes':
      return v === true ? { allHomes: true } : null
    case 'allServers':
      return v === true ? { allServers: true } : null
    default:
      return null
  }
}

function parseEntries(raw: unknown): ZenGrantEntry[] {
  if (!Array.isArray(raw)) return []
  const out: ZenGrantEntry[] = []
  for (const e of raw) {
    if (isObj(e) && typeof e.server === 'string' && typeof e.room === 'string') out.push({ server: e.server, room: e.room })
  }
  return out
}

function parsePause(raw: unknown): ZenGrantPause | null {
  if (!isObj(raw) || raw.reason !== 'runaway') return null
  return { at: str(raw.at) ?? '', reason: 'runaway' }
}

/** `grant` on a custom widget (Rust `GrantView`), or null when absent. An
 *  unreadable state reads as `invalid` (fails closed: no caps). */
export function parseZenGrantView(raw: unknown): ZenGrantView | null {
  if (!isObj(raw)) return null
  const state = GRANT_STATES.includes(raw.state as ZenGrantState) ? (raw.state as ZenGrantState) : 'invalid'
  const closed = state === 'none' || state === 'invalid' || state === 'review'
  return {
    state,
    caps: closed ? [] : zenWidgetCaps(raw.caps),
    granted: zenWidgetCaps(raw.granted),
    scope: parseZenScope(raw.scope),
    entries: parseEntries(raw.entries),
    sending: raw.sending === true,
    paused: parsePause(raw.paused),
    grantedAt: str(raw.grantedAt),
    widgetHash: str(raw.widgetHash),
  }
}

function parseFindings(raw: unknown): ZenWidgetFinding[] {
  if (!Array.isArray(raw)) return []
  const out: ZenWidgetFinding[] = []
  for (const f of raw) {
    if (!isObj(f) || typeof f.message !== 'string') continue
    out.push({
      file: str(f.file) ?? '',
      line: typeof f.line === 'number' ? f.line : 0,
      col: typeof f.col === 'number' ? f.col : 0,
      message: f.message,
    })
  }
  return out
}

function parseConfig(raw: unknown): Record<string, string | number | boolean> {
  const out: Record<string, string | number | boolean> = {}
  if (!isObj(raw)) return out
  for (const [k, v] of Object.entries(raw)) {
    if (typeof v === 'string' || typeof v === 'boolean' || (typeof v === 'number' && Number.isFinite(v))) out[k] = v
  }
  return out
}

function parseLibs(raw: unknown): ZenLibRef[] {
  if (!Array.isArray(raw)) return []
  const out: ZenLibRef[] = []
  for (const l of raw) {
    if (typeof l === 'string' && l) out.push(l)
    else if (isObj(l) && typeof l.url === 'string' && typeof l.integrity === 'string') out.push({ url: l.url, integrity: l.integrity })
  }
  return out
}

function parseReasons(raw: unknown): Partial<Record<UserWidgetCap, string>> {
  const out: Partial<Record<UserWidgetCap, string>> = {}
  if (!isObj(raw)) return out
  for (const [k, v] of Object.entries(raw)) {
    if (WIDGET_CAPS.has(k) && typeof v === 'string') out[k as UserWidgetCap] = v
  }
  return out
}

/**
 * One custom widget from `page.widgets` (`kind: "custom"`). `base` is what
 * the page parser already read (id, column, props). The effective caps are
 * intersected again here (UW26 step 2): requested ∩ the grant's caps ∩ the
 * four widget caps, and none while the grant needs review.
 */
export function parseZenCustomWidget(
  raw: Record<string, unknown>,
  base: { id: string; column: number; props: Record<string, unknown> },
): ZenCustomWidgetPayload {
  const requested = zenWidgetCaps(raw.requested)
  const grant = parseZenGrantView(raw.grant)
  const fromDaemon = zenWidgetCaps(raw.caps)
  const grantCaps = grant ? grant.caps : []
  const caps = requested.filter((c) => fromDaemon.includes(c) && grantCaps.includes(c))
  const props = base.props
  const state = raw.state === 'ok' || raw.state === 'errors' ? raw.state : 'broken'
  const name = str(raw.name)?.trim()
  return {
    id: base.id,
    kind: 'custom',
    widget: str(raw.widget) ?? '',
    column: base.column,
    props: {
      ...(typeof props.home === 'string' && props.home.trim() ? { home: props.home.trim() } : {}),
      ...(typeof props.agent === 'string' && props.agent.trim() ? { agent: props.agent.trim() } : {}),
      config: parseConfig(props.config),
    },
    caps,
    requested,
    source: 'user',
    name: name || str(raw.widget) || base.id,
    description: str(raw.description),
    reasons: parseReasons(raw.reasons),
    libs: parseLibs(raw.libs),
    hash: str(raw.hash) ?? '',
    state,
    errors: parseFindings(raw.errors),
    warnings: parseFindings(raw.warnings),
    grant,
  }
}

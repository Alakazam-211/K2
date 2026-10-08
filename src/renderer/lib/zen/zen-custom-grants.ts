// prd-zen-user-widgets-v2 UW35, UWB3, UWB4, UWB6, UWB9 — custom-widget
// routes on THIS computer's daemon.
//
// Grants live in the local daemon (signed there, UWB4). These routes take
// only the owner token (UWB3), which is what `zenLocalScope()` carries; a
// passport, a Connect login or an app pass gets 403 `owner_only`. So only
// this app's own UI, the Allow click, can grant:
//   GET  /cli/zen/widget/bundle?widget=<name>  the last good bundle (nonced)
//   POST /cli/zen/widget/grant                 Allow (scope, entries, sending, hash)
//   POST /cli/zen/widget/revoke                Turn off (one placement or a widget)
//   POST /cli/zen/widget/sending               Sending on/off (off: also the runaway guard)
//   POST /cli/zen/widget/resume                clear a runaway pause
//   GET  /cli/zen/widget/grants                Settings → Gardens list
// Every body is parsed here. Errors carry a code the UI words.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { zenLocalScope, isZenRouteMissing } from './zen-api'
import { parseZenScope, zenWidgetCaps } from './zen-custom-payload'
import type {
  ZenGrantState,
  ZenWidgetBundleResponse,
  ZenWidgetGrantRequest,
  ZenWidgetGrantRow,
  ZenWidgetResumeRequest,
  ZenWidgetRevokeRequest,
  ZenWidgetSendingRequest,
} from './zen-custom-types'

export type ZenWidgetErrorCode =
  | 'older_daemon' // no `zen-widgets-v1` routes (UW36)
  | 'widget_changed' // 409: the bundle changed while reviewing (UW22)
  | 'widget_broken' // 409: no good bundle yet (UW11)
  | 'unknown_widget' // 404
  | 'owner_only' // 403: not the owner's own app (UWB3)
  | 'failed'

export class ZenWidgetError extends Error {
  readonly code: ZenWidgetErrorCode
  constructor(code: ZenWidgetErrorCode, message: string) {
    super(message)
    this.name = 'ZenWidgetError'
    this.code = code
  }
}

const CODES: ReadonlyArray<[RegExp, ZenWidgetErrorCode]> = [
  [/widget_changed/, 'widget_changed'],
  [/widget_broken/, 'widget_broken'],
  [/unknown_widget/, 'unknown_widget'],
  [/owner_only/, 'owner_only'],
]

/** The words a person reads for each refusal. */
export function zenWidgetErrorText(code: ZenWidgetErrorCode, raw: string): string {
  switch (code) {
    case 'older_daemon':
      return 'K2 on this computer is older than this app. Update it to run custom widgets.'
    case 'widget_changed':
      return 'The widget changed while you were reviewing it. Review it again.'
    case 'widget_broken':
      return 'This widget has errors and no working version yet.'
    case 'unknown_widget':
      return 'This widget’s folder is gone.'
    case 'owner_only':
      return 'Only you can allow a widget, from the K2 app on this computer.'
    default:
      return raw || 'Something went wrong.'
  }
}

/** Map a daemon error to a `ZenWidgetError`. */
export function zenWidgetError(err: unknown): ZenWidgetError {
  if (err instanceof ZenWidgetError) return err
  const raw = err instanceof Error ? err.message : String(err)
  if (isZenRouteMissing(err)) return new ZenWidgetError('older_daemon', zenWidgetErrorText('older_daemon', raw))
  for (const [re, code] of CODES) if (re.test(raw)) return new ZenWidgetError(code, zenWidgetErrorText(code, raw))
  return new ZenWidgetError('failed', zenWidgetErrorText('failed', raw))
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** Parse `GET widget/bundle`. Throws on anything that isn't a bundle. */
export function parseZenWidgetBundle(raw: unknown): ZenWidgetBundleResponse {
  if (
    !isObj(raw) ||
    typeof raw.widget !== 'string' ||
    typeof raw.hash !== 'string' ||
    typeof raw.nonce !== 'string' ||
    typeof raw.html !== 'string'
  ) {
    throw new ZenWidgetError('failed', 'The widget bundle answer is not readable.')
  }
  // A nonce is CSP source text: base64/hex only, or the frame's policy breaks.
  if (!/^[A-Za-z0-9+/=_-]{16,128}$/.test(raw.nonce)) {
    throw new ZenWidgetError('failed', 'The widget bundle nonce is not usable.')
  }
  return {
    ok: true,
    widget: raw.widget,
    hash: raw.hash,
    nonce: raw.nonce,
    html: raw.html,
    bytes: typeof raw.bytes === 'number' ? raw.bytes : raw.html.length,
    ...(raw.state === 'ok' || raw.state === 'errors' || raw.state === 'broken' ? { state: raw.state } : {}),
  }
}

export async function fetchZenWidgetBundle(widget: string): Promise<ZenWidgetBundleResponse> {
  try {
    return parseZenWidgetBundle(await daemonCliGet<unknown>(zenLocalScope(), 'zen/widget/bundle', { widget }))
  } catch (err) {
    throw zenWidgetError(err)
  }
}

async function post(route: string, body: object): Promise<unknown> {
  try {
    return await daemonCliPost<unknown>(zenLocalScope(), route, body)
  } catch (err) {
    throw zenWidgetError(err)
  }
}

/** Allow (UW22 step 3). The daemon re-reads the placement's ask itself. */
export async function grantZenWidget(req: ZenWidgetGrantRequest): Promise<void> {
  if (req.entries.length === 0) throw new ZenWidgetError('failed', 'There are no agents there yet.')
  await post('zen/widget/grant', req)
}

/** Turn off (UW25): always allowed. */
export async function revokeZenWidget(req: ZenWidgetRevokeRequest): Promise<void> {
  await post('zen/widget/revoke', req)
}

/** The Sending switch (UWB9). */
export async function setZenWidgetSending(req: ZenWidgetSendingRequest): Promise<void> {
  await post('zen/widget/sending', req)
}

/** Resume after the runaway guard (owner only). */
export async function resumeZenWidget(req: ZenWidgetResumeRequest): Promise<void> {
  await post('zen/widget/resume', req)
}

const GRANT_STATES: readonly ZenGrantState[] = ['none', 'invalid', 'review', 'partial', 'granted']

/** Parse `GET widget/grants`: `{ok, grants: [...]}`. Unreadable rows are dropped. */
export function parseZenWidgetGrants(raw: unknown): ZenWidgetGrantRow[] {
  if (!isObj(raw) || !Array.isArray(raw.grants)) throw new ZenWidgetError('failed', 'The grants answer is not readable.')
  const out: ZenWidgetGrantRow[] = []
  for (const g of raw.grants) {
    if (!isObj(g) || typeof g.garden !== 'string' || typeof g.placement !== 'string' || typeof g.widget !== 'string') continue
    const scope = parseZenScope(g.scope)
    if (!scope) continue
    out.push({
      garden: g.garden,
      placement: g.placement,
      widget: g.widget,
      state: GRANT_STATES.includes(g.state as ZenGrantState) ? (g.state as ZenGrantState) : 'invalid',
      caps: zenWidgetCaps(g.caps),
      scope,
      entries: Array.isArray(g.entries)
        ? g.entries.flatMap((e) => (isObj(e) && typeof e.server === 'string' && typeof e.room === 'string' ? [{ server: e.server, room: e.room }] : []))
        : [],
      sending: g.sending === true,
      paused: isObj(g.paused) && g.paused.reason === 'runaway' ? { at: typeof g.paused.at === 'string' ? g.paused.at : '', reason: 'runaway' } : null,
      grantedAt: typeof g.grantedAt === 'string' ? g.grantedAt : '',
      ...(typeof g.gardenName === 'string' && g.gardenName ? { gardenName: g.gardenName } : {}),
    })
  }
  return out
}

export async function listZenWidgetGrants(): Promise<ZenWidgetGrantRow[]> {
  try {
    return parseZenWidgetGrants(await daemonCliGet<unknown>(zenLocalScope(), 'zen/widget/grants'))
  } catch (err) {
    throw zenWidgetError(err)
  }
}

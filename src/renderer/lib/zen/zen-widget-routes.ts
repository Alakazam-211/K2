// prd-zen-user-widgets-v2 UW35, UWB9 — custom-widget routes on THIS
// computer's daemon. There are no permission routes: a widget in your own
// Garden runs with every Garden-safe cap it asks for (Rosson 2026-10-08).
//   GET  /cli/zen/widget/bundle?widget=<name>&garden=<id>  the last good bundle
//                                              (nonced); `garden` lets a locked
//                                              Garden get its frozen copy
//   POST /cli/zen/widget/pause                 the runaway guard tripped
//   POST /cli/zen/widget/resume                the person's Resume click
//                                              (owner token only)
//   POST /cli/zen/widget/run                   a frame's ready or uncaught error
// Every body is parsed here. Errors carry a code the UI words.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { zenLocalScope, isZenRouteMissing } from './zen-api'
import type { ZenWidgetBundleResponse, ZenWidgetPauseRequest, ZenWidgetResumeRequest } from './zen-custom-types'

export type ZenWidgetErrorCode =
  | 'older_daemon' // no `zen-widgets-v1` routes (UW36)
  | 'widget_broken' // 409: no good bundle yet (UW11)
  | 'unknown_widget' // 404
  | 'owner_only' // 403: Resume from anything but the owner's own app
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
  [/widget_broken/, 'widget_broken'],
  [/unknown_widget/, 'unknown_widget'],
  [/owner_only/, 'owner_only'],
]

/** The words a person reads for each refusal. */
export function zenWidgetErrorText(code: ZenWidgetErrorCode, raw: string): string {
  switch (code) {
    case 'older_daemon':
      return 'K2 on this computer is older than this app. Update it to run custom widgets.'
    case 'widget_broken':
      return 'This widget has errors and no working version yet.'
    case 'unknown_widget':
      return 'This widget’s folder is gone.'
    case 'owner_only':
      return 'Only you can resume a widget, from the K2 app on this computer.'
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

/** The live bundle. With `garden`, a locked Garden gets the copy frozen when
 *  it was locked (prd-garden-lock-and-sync-v1); older daemons ignore it. */
export async function fetchZenWidgetBundle(widget: string, garden?: string): Promise<ZenWidgetBundleResponse> {
  try {
    const params = garden ? { widget, garden } : { widget }
    return parseZenWidgetBundle(await daemonCliGet<unknown>(zenLocalScope(), 'zen/widget/bundle', params))
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

/** The runaway guard tripped (R6): posting pauses in every window. */
export async function pauseZenWidget(req: ZenWidgetPauseRequest): Promise<void> {
  await post('zen/widget/pause', req)
}

/** One frame's `ready` or uncaught error, for the bundle `hash` it ran
 *  (decision-k2-runtime-v1 stage 0 (d)): the daemon logs it so an agent
 *  sees it with `k2 zen widget errors <name>` and `k2 zen validate`.
 *  User widgets only: K2's built-ins (`k2:…`) aren't logged. */
export interface ZenWidgetRunReport {
  widget: string
  hash: string
  event: 'ready' | 'error'
  message?: string
  stack?: string
  /** Garden v3 (GV49): fixed fields; all a `sources:read` widget sends. */
  kind?: 'error' | 'refusal'
  class?: string
  code?: string
  line?: number
}

/** `TypeError` from `TypeError: x is undefined` (a JS identifier only). */
export function zenErrorClass(message: string): string | undefined {
  const m = /^([A-Za-z_$][A-Za-z0-9_$]{0,59}):/.exec(message.trim())
  return m ? m[1] : undefined
}

/** The first `:line:col` in a stack. */
export function zenErrorLine(stack: string | undefined): number | undefined {
  const m = stack ? /:(\d{1,7}):\d{1,7}/.exec(stack) : null
  return m ? Number(m[1]) : undefined
}

/** The body for one frame report. A widget that reads data sources sends
 *  its class, code and line only, never message text (GV49: a card could
 *  throw a value into text agents read); the daemon drops it again. */
export function zenRunReportBody(
  widget: string,
  hash: string,
  r: { event: 'ready' } | { event: 'error'; message: string; stack?: string } | { event: 'refusal'; verb: string; code: string },
  readsSources: boolean,
): ZenWidgetRunReport {
  if (r.event === 'ready') return { widget, hash, event: 'ready' }
  if (r.event === 'refusal') return { widget, hash, event: 'error', kind: 'refusal', class: r.verb, code: r.code }
  const cls = zenErrorClass(r.message)
  const line = zenErrorLine(r.stack)
  const fixed = { kind: 'error' as const, ...(cls ? { class: cls } : {}), ...(line !== undefined ? { line } : {}) }
  if (readsSources) return { widget, hash, event: 'error', ...fixed }
  return { widget, hash, event: 'error', message: r.message, ...(r.stack ? { stack: r.stack } : {}), ...fixed }
}

export function zenWidgetRunLogged(widget: string): boolean {
  return !widget.startsWith('k2:')
}

export async function reportZenWidgetRun(req: ZenWidgetRunReport): Promise<void> {
  if (!zenWidgetRunLogged(req.widget)) return
  await post('zen/widget/run', req)
}

/** "Paused: too many posts. Resume" (owner only). */
export async function resumeZenWidget(req: ZenWidgetResumeRequest): Promise<void> {
  await post('zen/widget/resume', req)
}

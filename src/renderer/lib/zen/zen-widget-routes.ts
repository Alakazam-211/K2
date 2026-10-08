// prd-zen-user-widgets-v2 UW35, UWB9 — custom-widget routes on THIS
// computer's daemon. There are no permission routes: a widget in your own
// Garden runs with every Garden-safe cap it asks for (Rosson 2026-10-08).
//   GET  /cli/zen/widget/bundle?widget=<name>  the last good bundle (nonced)
//   POST /cli/zen/widget/pause                 the runaway guard tripped
//   POST /cli/zen/widget/resume                the person's Resume click
//                                              (owner token only)
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

/** The runaway guard tripped (R6): posting pauses in every window. */
export async function pauseZenWidget(req: ZenWidgetPauseRequest): Promise<void> {
  await post('zen/widget/pause', req)
}

/** "Paused: too many posts. Resume" (owner only). */
export async function resumeZenWidget(req: ZenWidgetResumeRequest): Promise<void> {
  await post('zen/widget/resume', req)
}

// The daemon's machine-readable refusals: `{"error":{"code","hint"}}`
// (vault routes, `thread/answer`).
//
// Two shapes reach here. `daemonCliGet/Post` throw a `DaemonHttpError`
// whose message is the hint and whose `code` is the body's code (since
// 0.46.1, `daemon-cli` unwraps the `{error:{code,hint}}` shape). Other
// callers (tests, older paths) throw an Error whose message is the raw
// body. Both are read here. Reading only the raw body broke every
// two-step refusal in 0.46.1/0.46.2 (the secret card's "Save anyway",
// the API token vault confirm): the code was never seen.

import { httpCodeOf } from './daemon-http-error'

export interface DaemonErrorInfo {
  code: string
  hint: string
}

/** The `{error:{code,hint}}` inside a thrown daemon error, or null. */
export function daemonErrorOf(e: unknown): DaemonErrorInfo | null {
  const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : ''
  const fromBody = errorInBody(msg)
  if (fromBody) return fromBody
  // `daemon-cli`'s DaemonHttpError: the code rides the error, the message
  // is the hint.
  const code = httpCodeOf(e)
  if (code) return { code, hint: msg === code ? '' : msg }
  return null
}

function errorInBody(msg: string): DaemonErrorInfo | null {
  const start = msg.indexOf('{')
  if (start < 0) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(msg.slice(start))
  } catch {
    return null
  }
  const err = (parsed as { error?: unknown } | null)?.error
  if (!err || typeof err !== 'object') return null
  const { code, hint } = err as { code?: unknown; hint?: unknown }
  if (typeof code !== 'string') return null
  return { code, hint: typeof hint === 'string' ? hint : '' }
}

/** What a person reads for a failed daemon call. */
export function daemonErrorText(e: unknown): string {
  const info = daemonErrorOf(e)
  if (info) return info.hint || info.code
  return e instanceof Error ? e.message : String(e)
}

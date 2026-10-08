// The daemon's machine-readable refusals: `{"error":{"code","hint"}}`
// (vault routes, `thread/answer`). `daemonCliGet/Post` throw an Error whose
// message is the raw body for this shape (it only unwraps a string `error`),
// so this reads the code and hint back out of it.

export interface DaemonErrorInfo {
  code: string
  hint: string
}

/** The `{error:{code,hint}}` inside a thrown daemon error, or null. */
export function daemonErrorOf(e: unknown): DaemonErrorInfo | null {
  const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : ''
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

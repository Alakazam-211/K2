// A non-2xx answer from a daemon `/cli/*` route, as `daemon-cli` throws it:
// the message (the body's `error`, else the raw text) plus the HTTP status
// and the body's `code` when it has one.
//
// Usage v2 (B25): callers branch on the status (403 = this login's role may
// not see it), never on the message text. Kept apart from `daemon-cli` so a
// test that mocks `daemon-cli` still gets the real helpers here.

export class DaemonHttpError extends Error {
  readonly status: number
  readonly code: string | null
  constructor(message: string, status: number, code: string | null) {
    super(message)
    this.status = status
    this.code = code
  }
}

/** The HTTP status of a daemon answer an error came from, or null when the
 *  error was not an HTTP answer (network, host switch, parse). */
export function httpStatusOf(err: unknown): number | null {
  if (err === null || typeof err !== 'object') return null
  const status = (err as { status?: unknown }).status
  return typeof status === 'number' && Number.isInteger(status) ? status : null
}

/** The daemon's refusal `code` (`admin_only`, `own_workspace_only`, …), or null. */
export function httpCodeOf(err: unknown): string | null {
  if (err === null || typeof err !== 'object') return null
  const code = (err as { code?: unknown }).code
  return typeof code === 'string' && code !== '' ? code : null
}

/** The message of any thrown value. */
export function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

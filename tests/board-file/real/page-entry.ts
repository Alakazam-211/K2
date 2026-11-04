// What the real-file specs load into the page: the real session, and the daemon's answers read as daemon-cli reads them.
import { createBoardFilePartJoiner, createBoardFileSession } from '../../../src/renderer/lib/board-file/board-file-session'
import { DaemonHttpError } from '../../../src/renderer/lib/daemon-http-error'

/** A response as daemon-cli's `parseDaemonResponse` and `httpError` turn it into a value or a DaemonHttpError. */
async function answer(r: Response): Promise<unknown> {
  const text = await r.text()
  const body = JSON.parse(text) as Record<string, unknown>
  if (r.ok) return body
  const e = body.error
  const err = e && typeof e === 'object' ? (e as Record<string, unknown>) : null
  const code = err && typeof err.code === 'string' ? err.code : typeof body.code === 'string' ? body.code : null
  const msg = err && typeof err.hint === 'string' ? err.hint : typeof e === 'string' ? e : text
  throw new DaemonHttpError(msg, r.status, code, err, body)
}

;(window as unknown as { BF: unknown }).BF = { createBoardFileSession, createBoardFilePartJoiner, answer }

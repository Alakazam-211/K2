// When the file viewer must NOT let you edit (and so must never write).
//
// `fs/read-file` refuses a file it can't return as text. Only a refusal
// that means "no such file" may open as an empty, editable new file. Any
// other refusal (binary, not UTF-8, over the 10 MB text cap, permission,
// network) means a real file may be there: an edit followed by Save or
// the autosave-on-leave would overwrite it with the typed text. So the
// pane locks instead.
//
// The daemon names the reason in the 400 body's `code` (k2-core
// `ReadFileError`). An older daemon sends no code; then only its exact
// not-found messages count as not-found, and everything else locks.
//
// Pure (no React) so it is unit-testable and cheap to import.

import { errorMessage, httpCodeOf } from '@/lib/daemon-http-error'

export interface TextLock {
  /** `unreadable`: the file is there but isn't text K2 can show
   *  (binary, not UTF-8, too large). `failed`: the read failed for some
   *  other reason (permission, network, …). */
  reason: 'unreadable' | 'failed'
  /** The daemon's message (or ours), shown under the headline. */
  detail: string
}

/** Codes from `/cli/fs/read-file` for a file that exists but isn't text. */
const UNREADABLE_CODES = new Set(['binary', 'not_utf8', 'too_large'])

export const TEXT_LOCK_MESSAGES: Record<TextLock['reason'], string> = {
  unreadable:
    "K2 can't open this file as text (it's binary, too large, or not UTF-8). Editing is turned off so the file isn't overwritten.",
  failed:
    "K2 couldn't read this file, so editing is turned off and the file won't be overwritten. Use Refresh to try again.",
}

/** Does this `fs/read-file` failure mean the file does not exist yet? */
export function isNotFoundReadError(err: unknown): boolean {
  const code = httpCodeOf(err)
  if (code !== null) return code === 'not_found'
  // An older daemon (no `code`): its exact not-found messages only.
  const msg = errorMessage(err)
  return msg === 'File not found' || msg.startsWith('Parent directory does not exist')
}

/** The lock for a failed `fs/read-file`, or null when the file simply
 *  doesn't exist (the pane opens it as a new, editable file). */
export function textLockForReadError(err: unknown): TextLock | null {
  if (isNotFoundReadError(err)) return null
  const code = httpCodeOf(err)
  const detail = errorMessage(err)
  const unreadable = code !== null ? UNREADABLE_CODES.has(code) : /binary|too large/i.test(detail)
  return { reason: unreadable ? 'unreadable' : 'failed', detail }
}

/** The lock for a read that succeeded but returned binary-looking text. */
export function textLockForBinaryContent(): TextLock {
  return { reason: 'unreadable', detail: 'The file contains NUL bytes, so it looks binary.' }
}

// When the file viewer must NOT let you edit (and so must never write).
//
// `fs/read-file` refuses a file it can't return as text. Only a refusal
// that means "no such file" may open as an empty, editable new file. Any
// other refusal on the FIRST load (binary, not UTF-8, over the 10 MB text
// cap, permission, network) means a real file may be there: an edit
// followed by Save or the autosave-on-leave would overwrite it with the
// typed text. So the pane locks instead.
//
// After a file has been read as text once, only a content-type refusal
// (`binary`, `not_utf8`, `too_large`, or binary-looking text) locks: the
// file on disk is no longer text. A transient failure (permission, I/O,
// network, 5xx, or the file vanishing) keeps the buffer and its edits;
// the pane shows a banner instead.
//
// The daemon names the reason in the 400 body's `code` (k2-core
// `ReadFileError`). An older daemon sends no code; then only its exact
// not-found messages count as not-found, and only its binary / too-large
// messages count as content-type refusals.
//
// Pure (no React) so it is unit-testable and cheap to import.

import { errorMessage, httpCodeOf } from '@/lib/daemon-http-error'

export interface TextLock {
  /** `unreadable`: the file is there but isn't text K2 can show
   *  (binary, not UTF-8, too large). `failed`: the first read failed for
   *  some other reason (permission, network, …). */
  reason: 'unreadable' | 'failed'
  /** The daemon's message (or ours), shown under the headline. */
  detail: string
  /** Unsaved edits the person had when the file on disk stopped being
   *  text. Shown read-only so they can copy them out; never written. */
  keptText: string | null
}

/** Codes from `/cli/fs/read-file` for a file that exists but isn't text. */
const UNREADABLE_CODES = new Set(['binary', 'not_utf8', 'too_large'])

export const TEXT_LOCK_MESSAGES: Record<TextLock['reason'], string> = {
  unreadable:
    "K2 can't open this file as text (it's binary, too large, or not UTF-8). Editing is turned off so the file isn't overwritten.",
  failed:
    "K2 couldn't read this file, so editing is turned off and the file won't be overwritten. Use Refresh to try again.",
}

/** Lock headline when the person had unsaved edits as the file changed. */
export const TEXT_LOCK_KEPT_MESSAGE =
  "The file on disk changed to a format K2 can't edit. Your unsaved text is kept here; copy it out if you need it."

/** Banner when a re-read fails but the buffer stays. */
export const REREAD_FAILED_MESSAGE = "Couldn't re-read the file from disk."
export const REREAD_KEPT_MESSAGE = 'Your unsaved changes are kept.'
export const REREAD_DELETED_MESSAGE =
  "The file was deleted or moved outside K2, so K2 won't recreate it unless you save."

/** Does this `fs/read-file` failure mean the file does not exist (now)? */
export function isNotFoundReadError(err: unknown): boolean {
  const code = httpCodeOf(err)
  if (code !== null) return code === 'not_found'
  // An older daemon (no `code`): its exact not-found messages only.
  const msg = errorMessage(err)
  return msg === 'File not found' || msg.startsWith('Parent directory does not exist')
}

/** Does this failure mean the file exists but isn't text K2 can show? */
export function isContentTypeReadError(err: unknown): boolean {
  const code = httpCodeOf(err)
  if (code !== null) return UNREADABLE_CODES.has(code)
  return /binary|too large/i.test(errorMessage(err))
}

/** The lock for a failed FIRST `fs/read-file`, or null when the file
 *  simply doesn't exist (the pane opens it as a new, editable file). */
export function textLockForReadError(err: unknown): TextLock | null {
  if (isNotFoundReadError(err)) return null
  return {
    reason: isContentTypeReadError(err) ? 'unreadable' : 'failed',
    detail: errorMessage(err),
    keptText: null,
  }
}

/** The lock for a read that succeeded but returned binary-looking text. */
export function textLockForBinaryContent(keptText: string | null = null): TextLock {
  return { reason: 'unreadable', detail: 'The file contains NUL bytes, so it looks binary.', keptText }
}

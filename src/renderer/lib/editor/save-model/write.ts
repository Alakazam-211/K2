// The checked write (build-editors.md §3; wire-shapes "fs/write-file
// expect"): every save sends `expect`, the SHA-256 (64 lowercase hex) of the
// text K2 last read (fs/read-file's content is the file's UTF-8 as is, so its
// hash is the file's), or "absent" for a file K2 saw missing. On a mismatch
// nothing is written and the daemon answers 409 {code, current?}:
//   file_changed  another save landed; current {sha256, content?}
//   file_missing  the file is gone
//   file_exists   it expected none and one is there; current as above
// An App guest gets the hash only (no content), so K2 can't merge for them.

import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { clearDraft, putDraft } from './drafts'
import type { ConflictCode } from './reducer'
import { PRODUCT_NAME } from '@/lib/brand'

export const EXPECT_ABSENT = 'absent'

export async function sha256Hex(text: string): Promise<string> {
  const bytes = new TextEncoder().encode(text)
  const digest = await globalThis.crypto.subtle.digest('SHA-256', bytes)
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('')
}

/** `expect` for a base: its hash, or "absent" when K2 saw no file. */
export const expectFor = async (base: string | null): Promise<string> => (base === null ? EXPECT_ABSENT : sha256Hex(base))

export interface CurrentFile {
  sha256: string
  /** Missing for an App guest, or when the daemon couldn't read it as text. */
  content?: string
}

export type WriteResult = { ok: true } | { ok: false; code: ConflictCode; current: CurrentFile | null }

const CODES: readonly string[] = ['file_changed', 'file_missing', 'file_exists']

/** A refusal of the disk check, or null for any other error. */
export function conflictOf(err: unknown): { code: ConflictCode; current: CurrentFile | null } | null {
  if (!err || typeof err !== 'object') return null
  const e = err as { status?: number; code?: string | null; body?: unknown }
  if (e.status !== 409 || !e.code || !CODES.includes(e.code)) return null
  const body = (e.body ?? {}) as { current?: { sha256?: unknown; content?: unknown } }
  const c = body.current
  const current = c && typeof c.sha256 === 'string' ? { sha256: c.sha256, ...(typeof c.content === 'string' ? { content: c.content } : {}) } : null
  return { code: e.code as ConflictCode, current }
}

/** Write `text` to `path` only if the file is still `base` (null: still absent).
 *  Resolves to the disk check's refusal; throws any other failure. */
export async function writeChecked(scope: ServerScope, path: string, text: string, base: string | null): Promise<WriteResult> {
  const expect = await expectFor(base)
  try {
    await daemonCliPost(scope, 'fs/write-file', { path, content: text, expect })
    return { ok: true }
  } catch (err) {
    const c = conflictOf(err)
    if (c) return { ok: false, ...c }
    throw err
  }
}

/** The tab closed (or the workspace switched) with unsaved words: the same
 *  checked write. If it lands the draft goes; if not (an outside save, a
 *  missing file, any failure) nothing is overwritten and the words stay as
 *  this computer's draft, so the file's next open brings them back. */
export async function leaveWrite(scope: ServerScope, path: string, text: string, base: string | null): Promise<void> {
  try {
    const r = await writeChecked(scope, path, text, base)
    if (r.ok) {
      await clearDraft(scope.hostKey, path)
      return
    }
  } catch {
    /* kept below */
  }
  await putDraft(scope.hostKey, path, text, base)
}

/** Why a write failed, in words for the bar ("Couldn't save README.md: <reason>."). */
export function failureReason(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err ?? '')
  const m = raw.toLowerCase()
  if (/no space left|enospc|disk full|quota/.test(m)) return 'the drive is full'
  if (/read-only file system|erofs/.test(m)) return 'the drive is read-only'
  if (/permission denied|eacces|eperm|not permitted/.test(m)) return `${PRODUCT_NAME} isn’t allowed to write it`
  if (/timed? ?out|no answer from the server/.test(m)) return 'the server didn’t answer'
  if (/failed to fetch|network|econnrefused|offline|load failed/.test(m)) return 'the server can’t be reached'
  const clean = raw.trim().replace(/[.\s]+$/, '')
  if (!clean) return 'the server refused it'
  return clean.charAt(0).toLowerCase() + clean.slice(1)
}

/** What to do after the second failure, for the bar's muted line. */
export function failureHint(reason: string): string {
  if (reason === 'the drive is full') return 'Free up some space, then try again. Your words stay here.'
  if (reason === `${PRODUCT_NAME} isn’t allowed to write it`) return 'Check who can write the file, or save a copy. Your words stay here.'
  if (reason === 'the drive is read-only') return 'Save a copy somewhere you can write. Your words stay here.'
  if (reason.startsWith('the server')) return 'Check the connection, then try again. Your words stay here.'
  return 'Try again, or save a copy. Your words stay here.'
}

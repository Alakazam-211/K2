// Where the viewer gets a version's change, and how Undo reverts your latest.
// The change route isn't there yet: while a server doesn't have it (404, 405
// or 501), or answers with anything that isn't a change, the viewer draws as
// before, with no highlight. Undo reverts one version only while it is still
// the latest, against the drawing's current digest, and answers the undo's
// own change; each refusal comes back in plain words, and some read the
// drawing again or open it view only. Tests and shots use the fixture source.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { errorMessage, httpCodeOf, httpDetailsOf, httpStatusOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import { parseCadChange, type CadChange } from './cad-change'
import { noteWriterOff, WRITER_OFF_WORDS } from './edit/edit-source'
import { asOwn } from './edit/own-change'

export type RevertResult =
  /** `change`: the undo's own change (yours), when the server sends it. */
  | { ok: true; change: CadChange | null }
  /** `reload`: the drawing moved, read it again; `off`: the drawing can't be changed here, with these words. */
  | { ok: false; words: string; reload?: boolean; off?: string }

/** Undo's refusal when someone changed the drawing after the change: only the latest change can be undone. */
export const UNDO_NOT_LATEST_WORDS = 'Someone changed the drawing after this, so it can’t be undone.'
export const UNDO_WORDS = {
  stale: 'The drawing changed; reloaded.',
  noEarlier: 'There’s nothing earlier to go back to.',
  forbidden: 'You can’t change this drawing.',
  coreOff: 'Drawing edits are off on this server; this file opens read-only.',
  missing: 'Undo isn’t on this server yet.',
} as const

export interface CadChangeSource {
  /** What changed from version `from` to `to` of the file, or null when the server can't say. */
  change(path: string, from: number, to: number): Promise<CadChange | null>
  /** Undo your latest change (`version`, the head), against the head's digest (`sha256:…`). */
  revert(path: string, version: number, expectDigest: string): Promise<RevertResult>
}

// TODO: K2 adds the change route. `GET docs/cad/change {path, from, to}` answers
// the CAD pack's change between two versions (`cad.change/1`); until it does,
// other people's changes are read again with no outlines. `POST docs/cad/revert
// {path, change_version, expect_digest}` undoes that version while it is the
// head and the head's bytes hash to `expect_digest`, answering
// `{ok, path, reverted, to, version, digest, change}` at the top level.
export const CHANGE_ROUTE = 'docs/cad/change'
export const REVERT_ROUTE = 'docs/cad/revert'

const MISSING = new Set([404, 405, 501])

/** Undo's request body: the change to undo (the head) and the head's digest. */
export const revertBody = (path: string, version: number, expectDigest: string): Record<string, unknown> => ({
  path,
  change_version: version,
  expect_digest: expectDigest,
})

/** Undo's answer: the undo's change, as yours. */
export function revertDone(body: unknown): RevertResult {
  const b = body && typeof body === 'object' ? (body as Record<string, unknown>) : {}
  const c = b.change && typeof b.change === 'object' ? (b.change as Record<string, unknown>) : null
  const from = typeof c?.from === 'number' ? c.from : null
  const to = typeof c?.to === 'number' ? c.to : typeof b.version === 'number' ? b.version : null
  return { ok: true, change: c && to !== null ? asOwn(c, from ?? to - 1, to, 'undo') : null }
}

/** Undo's refusal, by its code, in plain words. */
export function revertRefused(e: unknown, onWriterOff?: (words: string) => void): RevertResult {
  const status = httpStatusOf(e)
  if (status !== null && MISSING.has(status)) return { ok: false, words: UNDO_WORDS.missing }
  const err = httpDetailsOf(e) ?? {}
  const hint = typeof err.hint === 'string' && err.hint.trim() ? err.hint.trim() : null
  switch (httpCodeOf(e)) {
    case 'cad_undo_not_latest':
      return { ok: false, words: UNDO_NOT_LATEST_WORDS }
    case 'cad_expect_stale':
      return { ok: false, words: UNDO_WORDS.stale, reload: true }
    case 'no_earlier_version':
      return { ok: false, words: UNDO_WORDS.noEarlier }
    case 'cad_writer_disabled': {
      const words = hint ?? WRITER_OFF_WORDS
      onWriterOff?.(words)
      return { ok: false, words, off: words }
    }
    case 'edit_forbidden':
      return { ok: false, words: UNDO_WORDS.forbidden }
    case 'core_off': {
      const words = hint ?? UNDO_WORDS.coreOff
      return { ok: false, words, off: words }
    }
    default:
      return { ok: false, words: errorMessage(e) }
  }
}

export function daemonCadChangeSource(scope: ServerScope): CadChangeSource {
  return {
    async change(path, from, to) {
      try {
        return parseCadChange(await daemonCliGet<unknown>(scope, CHANGE_ROUTE, { path, from, to }))
      } catch {
        // Missing route, or any other failure: no highlight, as before.
        return null
      }
    },
    async revert(path, version, expectDigest) {
      try {
        return revertDone(await daemonCliPost<unknown>(scope, REVERT_ROUTE, revertBody(path, version, expectDigest)))
      } catch (e) {
        return revertRefused(e, (words) => noteWriterOff(scope.hostKey, words))
      }
    },
  }
}

/**
 * Canned answers, for tests and shots: changes by `"<from>-<to>"` (or by the
 * version they end at), and Undo's answers by version (ok when not listed).
 */
export function fixtureCadChangeSource(
  changes: Record<string, unknown>,
  reverts: Record<number, RevertResult> = {},
): CadChangeSource & { calls: { change: [string, number, number][]; revert: [string, number, string][] } } {
  const calls = { change: [] as [string, number, number][], revert: [] as [string, number, string][] }
  return {
    calls,
    async change(path, from, to) {
      calls.change.push([path, from, to])
      return parseCadChange(changes[`${from}-${to}`] ?? changes[String(to)])
    },
    async revert(path, version, expectDigest) {
      calls.revert.push([path, version, expectDigest])
      return reverts[version] ?? { ok: true, change: null }
    },
  }
}

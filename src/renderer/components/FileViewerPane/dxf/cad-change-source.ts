// Where the viewer gets a version's change, and how its Undo reverts one.
// The daemon's routes for these aren't there yet: while a server doesn't
// have them (404, 405 or 501), or answers with anything that isn't a
// change, the viewer draws exactly as it did before, with no highlight.
// Tests and shots use the fixture source.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { errorMessage, httpStatusOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import { parseCadChange, type CadChange } from './cad-change'

export type RevertResult = { ok: true } | { ok: false; words: string }

export interface CadChangeSource {
  /** What changed from version `from` to `to` of the file, or null when the server can't say. */
  change(path: string, from: number, to: number): Promise<CadChange | null>
  /** Undo one version only, leaving later changes by others in place. A refusal comes back in its own words. */
  revert(path: string, version: number): Promise<RevertResult>
}

// TODO: K2 adds these routes. `GET docs/cad/change {path, from, to}` answers the
// CAD pack's change between two versions (`cad.change/1`); `POST docs/cad/revert
// {path, version}` reverts that one version and refuses, with its reason in
// `error`, when a later change touched the same thing.
export const CHANGE_ROUTE = 'docs/cad/change'
export const REVERT_ROUTE = 'docs/cad/revert'

const MISSING = new Set([404, 405, 501])

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
    async revert(path, version) {
      try {
        await daemonCliPost<unknown>(scope, REVERT_ROUTE, { path, version })
        return { ok: true }
      } catch (e) {
        const status = httpStatusOf(e)
        if (status !== null && MISSING.has(status)) return { ok: false, words: 'Undo isn’t on this server yet.' }
        return { ok: false, words: errorMessage(e) }
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
): CadChangeSource & { calls: { change: [string, number, number][]; revert: [string, number][] } } {
  const calls = { change: [] as [string, number, number][], revert: [] as [string, number][] }
  return {
    calls,
    async change(path, from, to) {
      calls.change.push([path, from, to])
      return parseCadChange(changes[`${from}-${to}`] ?? changes[String(to)])
    },
    async revert(path, version) {
      calls.revert.push([path, version])
      return reverts[version] ?? { ok: true }
    },
  }
}

// Settings → Cores → Multiplayer (Beta), prd-multiplayer-cores-settings-v1
// §1, §2.2, §2.3, §3.
//
// Daemon-first: the list of cores, whether each is on, what is open live and
// who may change them all come from the focused server's daemon. This file
// reads that truth and posts one person's click back. Nothing is derived or
// kept here.
//
//   - Read: `GET /cli/docs/cores` (any logged-in role) →
//     `{ok, canChange, later, cores: [{id, name, status, description, view,
//     on, open: {files, participants, paths}, lastChange}]}`.
//   - Write: NOT a docs route (MC-2). `POST /cli/settings/update
//     {multiplayerCores: {<id>: <bool>}}`, one core per click. A Member gets
//     403 `updating "multiplayerCores" requires the Owner or Admin role`.
//   - Live: `doc_cores_changed` on the session event bus.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'

/** What is open live on one core right now. `paths`: at most 5,
 *  workspace-relative, only the caller's own readable ones. */
export interface DocCoreOpen {
  files: number
  participants: number
  paths: string[]
}

/** One path the last turn-off couldn't write back (§6.2). */
export interface DocCoreFailedPath {
  path: string
  error: string
}

/** The last time this core was switched, and what the switch did. */
export interface DocCoreLastChange {
  on: boolean
  by: string
  at: number
  closed: number
  written: number
  failed: DocCoreFailedPath[]
}

export interface DocCore {
  id: string
  name: string
  /** `beta` today. */
  status: string
  description: string
  /** `live`: people get the multiplayer view; `none`: agents only. */
  view: string
  on: boolean
  open: DocCoreOpen
  lastChange: DocCoreLastChange | null
}

export interface DocCores {
  canChange: boolean
  /** Cores still to come, plain words ("CSV grid", "Word"). */
  later: string[]
  cores: DocCore[]
}

/** Does the window's server have the multiplayer switches? */
export function multiplayerCoresAvailable(scope: ServerScope = primaryScope()): boolean {
  return scope.serverSupports('docs-cores-v1')
}

// ── Parsing (loud: a shape we don't know is an error, never a guess) ──

function obj(v: unknown, what: string): Record<string, unknown> {
  if (v === null || typeof v !== 'object' || Array.isArray(v)) throw new Error(`docs/cores: ${what} is not an object`)
  return v as Record<string, unknown>
}

function str(o: Record<string, unknown>, key: string, what: string): string {
  const v = o[key]
  if (typeof v !== 'string') throw new Error(`docs/cores: ${what}.${key} is not a string`)
  return v
}

function num(o: Record<string, unknown>, key: string, what: string): number {
  const v = o[key]
  if (typeof v !== 'number' || !Number.isFinite(v)) throw new Error(`docs/cores: ${what}.${key} is not a number`)
  return v
}

function bool(o: Record<string, unknown>, key: string, what: string): boolean {
  const v = o[key]
  if (typeof v !== 'boolean') throw new Error(`docs/cores: ${what}.${key} is not true or false`)
  return v
}

function strings(v: unknown, what: string): string[] {
  if (!Array.isArray(v) || v.some((x) => typeof x !== 'string')) throw new Error(`docs/cores: ${what} is not a list of strings`)
  return v as string[]
}

function parseOpen(v: unknown, what: string): DocCoreOpen {
  const o = obj(v, `${what}.open`)
  return {
    files: num(o, 'files', `${what}.open`),
    participants: num(o, 'participants', `${what}.open`),
    paths: strings(o.paths, `${what}.open.paths`),
  }
}

function parseLastChange(v: unknown, what: string): DocCoreLastChange | null {
  if (v === null || v === undefined) return null
  const o = obj(v, `${what}.lastChange`)
  const w = `${what}.lastChange`
  if (!Array.isArray(o.failed)) throw new Error(`docs/cores: ${w}.failed is not a list`)
  return {
    on: bool(o, 'on', w),
    by: str(o, 'by', w),
    at: num(o, 'at', w),
    closed: num(o, 'closed', w),
    written: num(o, 'written', w),
    failed: o.failed.map((f, i) => {
      const fo = obj(f, `${w}.failed[${i}]`)
      return { path: str(fo, 'path', `${w}.failed[${i}]`), error: str(fo, 'error', `${w}.failed[${i}]`) }
    }),
  }
}

/** `GET /cli/docs/cores`'s body. Throws on any field it can't read. */
export function parseDocCores(body: unknown): DocCores {
  const o = obj(body, 'the answer')
  if (!Array.isArray(o.cores)) throw new Error('docs/cores: "cores" is not a list')
  return {
    canChange: bool(o, 'canChange', 'the answer'),
    later: o.later === undefined ? [] : strings(o.later, '"later"'),
    cores: o.cores.map((c, i) => {
      const what = `cores[${i}]`
      const co = obj(c, what)
      return {
        id: str(co, 'id', what),
        name: str(co, 'name', what),
        status: str(co, 'status', what),
        description: str(co, 'description', what),
        view: str(co, 'view', what),
        on: bool(co, 'on', what),
        open: parseOpen(co.open, what),
        lastChange: parseLastChange(co.lastChange, what),
      }
    }),
  }
}

// ── Daemon calls (the focused server) ───────────────────────────────

export async function fetchDocCores(scope: ServerScope = primaryScope()): Promise<DocCores> {
  return parseDocCores(await daemonCliGet<unknown>(scope, 'docs/cores'))
}

/** Turn one core on or off. The body is exactly
 *  `{"multiplayerCores": {"<id>": <on>}}`. Throws the daemon's message on a
 *  refusal (403 for a Member, 400 for bad input). */
export async function setDocCore(id: string, on: boolean, scope: ServerScope = primaryScope()): Promise<void> {
  await daemonCliPost<unknown>(scope, 'settings/update', { multiplayerCores: { [id]: on } })
}

// ── Plain words ─────────────────────────────────────────────────────

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`
}

function pathsSuffix(paths: string[]): string {
  return paths.length > 0 ? ` (${paths.join(', ')})` : ''
}

/** "Open live now: 2 files (notes.md, plan.md), 3 people and agents." */
export function openNowLine(open: DocCoreOpen): string {
  return (
    `Open live now: ${plural(open.files, 'file', 'files')}${pathsSuffix(open.paths)}, ` +
    `${plural(open.participants, 'person or agent', 'people and agents')}.`
  )
}

/** A core in a sentence: "the Markdown core", or "the \"Any other file\" core"
 *  for a name that doesn't end in "core". */
export function coreInSentence(core: Pick<DocCore, 'name'>): string {
  return /\bcore$/i.test(core.name) ? `the ${core.name}` : `the "${core.name}" core`
}

/** The turn-off dialog's title: "Turn off the Markdown core?" */
export function turnOffTitle(core: Pick<DocCore, 'name'>): string {
  return `Turn off ${coreInSentence(core)}?`
}

/** The turn-off dialog's body. */
export function turnOffMessage(open: DocCoreOpen): string {
  const are = open.files === 1 ? 'is' : 'are'
  return (
    `${plural(open.files, 'file', 'files')} ${are} open live${pathsSuffix(open.paths)}. ` +
    'K2 writes every change to disk, closes them, and they reopen the usual way. Their history stays.'
  )
}

/** One line per path the last turn-off couldn't write back (§6.2). */
export function failedWriteBackLine(core: Pick<DocCore, 'name'>, f: DocCoreFailedPath): string {
  return (
    `${f.path} couldn't be written when ${coreInSentence(core)} was turned off (${f.error}). ` +
    `Its last changes are kept in K2's history; turning ${coreInSentence(core)} on again writes them.`
  )
}

// Where the viewer's edits go, whether the file can be changed at all, and
// who holds what. Every edit is one of the CAD pack's verbs, run through the
// server's doc tool route with `json: true`: the reply carries the kit's
// result object (`mpc.result/1`) under `result`, and a verb's own refusal is
// that result with `ok: false`. A session refusal comes back as HTTP 409
// with `error: {id | code, hint, holder | by, …}` (a lease, a hold), and a
// server that can't save drawing edits yet answers every edit with
// `cad_writer_disabled` and its own words: the viewer then shows the file
// view only and remembers that per server for the session. Holds come from
// who's on the file (agents' live claims, by the keys they cover) and are read
// again whenever a claim is taken or ends. Tests and shots use the fixture source.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { DaemonHttpError, errorMessage, httpCodeOf, httpDetailsOf, httpStatusOf } from '@/lib/daemon-http-error'
import { scopeMayWrite, type ServerScope } from '@/kessel/server-scope'
import { onDocStructureChanged } from '@/stores/session-events'
import { localRefusal, parseToolReply, parseToolResult, type ToolResult } from './tool-result'

/** An agent's hold: who, what for (the claim's note), and the keys it covers (`@h:<HANDLE>` of an entity, a layer, a block or a layout; `@doc:dxf` the drawing). */
export type Hold = { pid: string; name: string; note: string | null; keys: string[] }

export interface EditSource {
  /** May this viewer change the file at all (a view-only room may not). */
  readonly canWrite: boolean
  /** Run one verb's argv (its words, then the kit's flags) on the file. */
  run(path: string, argv: readonly string[]): Promise<ToolResult>
  /** Agents' holds on the file now; empty when the server can't say. */
  holds(path: string): Promise<Hold[]>
  /** Call `fn` whenever a claim on the file is taken or ends. */
  onHoldsChanged(path: string, fn: () => void): () => void
  /** Why the file opens read only, in the server's words, or null when it doesn't say so up front. */
  readOnly(path: string): Promise<string | null>
  /** This server refused to save a drawing edit this session: its words, or null. */
  writerOff(): string | null
}

/** The doc tool route: `{path, argv, json: true}` answers the kit's result object under `result`. */
export const TOOL_ROUTE = 'docs/tool'
/** Who's on the file, with each one's live claims. */
export const WHO_ROUTE = 'docs/who'
/** The file's core, with `read_only_reason` when it opens read only. */
export const CORES_ROUTE = 'docs/cores'

/** Refusals made here, never the kit's. */
export const NO_ROUTE = 'K2-NO-EDIT-ROUTE'
export const NOT_SENT = 'K2-NOT-SENT'
/** The server can't save drawing edits: the message is its own words. */
export const WRITER_OFF = 'K2-CAD-WRITER-OFF'
/** What a writer-off refusal says when the server gives no words. */
export const WRITER_OFF_WORDS = 'K2 can’t save drawing edits yet; this file opens read-only.'

const MISSING = new Set([404, 405, 501])
const KIT_ID = /^(MPC|DXF|CAD)-E\d+$/
/** A session refusal's code, until its id comes with it. */
const BY_CODE: Record<string, string> = { leased: 'MPC-E701', claimed: 'MPC-E301' }

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)
const text = (v: unknown): string | null => (typeof v === 'string' && v.trim() ? v.trim() : null)

/** A thrown answer, as a refusal: a writer that's off, a session refusal by its id (or code), else the kit's id when the server named one. */
export function refusalOfError(e: unknown): ToolResult {
  const status = httpStatusOf(e)
  if (status !== null && MISSING.has(status)) return localRefusal(NO_ROUTE)
  const err = httpDetailsOf(e) ?? {}
  const code = text(err.code) ?? httpCodeOf(e)
  if (code === 'cad_writer_disabled') return localRefusal(WRITER_OFF, text(err.hint) ?? WRITER_OFF_WORDS, err)
  // A reply that carries the kit's result as well: that result.
  const body = e instanceof DaemonHttpError ? e.body : null
  const fromBody = isObj(body) && isObj(body.result) ? parseToolReply(body) : null
  if (fromBody) return fromBody
  const id =
    (text(err.id) && KIT_ID.test(err.id as string) ? (err.id as string) : null) ??
    (code ? (BY_CODE[code] ?? (KIT_ID.test(code) ? code : null)) : null) ??
    /\b((?:MPC|DXF|CAD)-E\d+)\b/.exec(errorMessage(e))?.[1] ??
    NOT_SENT
  return localRefusal(id, text(err.hint) ?? errorMessage(e), err)
}

/** Agents' claims from `docs/who`'s roster: one hold per claim on nodes, with its holder and its note. */
export function parseHolds(body: unknown): Hold[] {
  const roster = isObj(body) && Array.isArray(body.roster) ? body.roster : []
  const out: Hold[] = []
  for (const e of roster) {
    if (!isObj(e) || e.kind !== 'agent' || typeof e.pid !== 'string') continue
    const name = typeof e.name === 'string' && e.name ? e.name : 'An agent'
    for (const c of Array.isArray(e.claims) ? e.claims : []) {
      if (!isObj(c) || !Array.isArray(c.nodes)) continue
      const keys = c.nodes.filter((k): k is string => typeof k === 'string')
      const note = typeof c.note === 'string' && c.note.trim() ? c.note.trim() : null
      if (keys.length) out.push({ pid: e.pid, name, note, keys })
    }
  }
  return out
}

/** `docs/cores?path=`'s read-only words for the file's core, read tolerantly. */
export function readOnlyOf(body: unknown): string | null {
  const file = isObj(body) && isObj(body.file) ? body.file : null
  return file ? (text(file.read_only_reason) ?? text(file.readOnlyReason)) : null
}

/** Whether a structure event's file is `path` (it names the file by its path in the workspace). */
export const sameFile = (path: string, rel: unknown): boolean =>
  typeof rel === 'string' && rel !== '' && (path === rel || path.endsWith(`/${rel.replace(/^\/+/, '')}`))

/** Servers that refused a drawing save this session, with their words. */
const writerOffBy = new Map<string, string>()

export function daemonEditSource(scope: ServerScope): EditSource {
  return {
    canWrite: scopeMayWrite(scope, TOOL_ROUTE),
    async run(path, argv) {
      try {
        return parseToolReply(await daemonCliPost<unknown>(scope, TOOL_ROUTE, { path, argv: [...argv], json: true })) ?? localRefusal(NO_ROUTE)
      } catch (e) {
        const r = refusalOfError(e)
        if (r.error?.id === WRITER_OFF) writerOffBy.set(scope.hostKey, r.error.message)
        return r
      }
    },
    async holds(path) {
      try {
        return parseHolds(await daemonCliGet<unknown>(scope, WHO_ROUTE, { path }))
      } catch {
        return []
      }
    },
    onHoldsChanged(path, fn) {
      return onDocStructureChanged(scope, (e) => {
        if ((e.change === 'claim_taken' || e.change === 'claim_ended') && sameFile(path, e.rel)) fn()
      })
    },
    async readOnly(path) {
      try {
        return readOnlyOf(await daemonCliGet<unknown>(scope, CORES_ROUTE, { path }))
      } catch {
        return null
      }
    },
    writerOff: () => writerOffBy.get(scope.hostKey) ?? null,
  }
}

/** For tests: forget which servers refused a drawing save. */
export function __resetWriterOffForTests(): void {
  writerOffBy.clear()
}

/** A source that answers from a function, for tests and shots; it records every call. */
export function fixtureEditSource(
  answer: (argv: readonly string[], n: number) => unknown,
  holds: Hold[] = [],
  opts: { readOnly?: string | null } = {},
): EditSource & { calls: string[][]; setHolds(h: Hold[]): void } {
  const calls: string[][] = []
  const listeners = new Set<() => void>()
  let now = holds
  let off: string | null = null
  return {
    canWrite: true,
    calls,
    setHolds: (h) => {
      now = h
      for (const fn of listeners) fn()
    },
    async run(_path, argv) {
      calls.push([...argv])
      const a = answer(argv, calls.length - 1)
      const r = a instanceof Error ? refusalOfError(a) : (parseToolResult(a) ?? localRefusal(NO_ROUTE))
      if (r.error?.id === WRITER_OFF) off = r.error.message
      return r
    },
    async holds() {
      return now
    },
    onHoldsChanged(_path, fn) {
      listeners.add(fn)
      return () => void listeners.delete(fn)
    },
    async readOnly() {
      return opts.readOnly ?? null
    },
    writerOff: () => off,
  }
}

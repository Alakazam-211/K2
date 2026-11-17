// Where the viewer's edits go, and where it learns who holds what. Every
// edit is one of the CAD pack's verbs, run through the server's doc tool
// route with `json: true`, so each outcome is the kit's result object
// (`mpc.result/1`). Holds come from who's on the file: an agent's live
// claims, by the keys they cover. Tests and shots use the fixture source.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { DaemonHttpError, errorMessage, httpCodeOf, httpStatusOf } from '@/lib/daemon-http-error'
import { scopeMayWrite, type ServerScope } from '@/kessel/server-scope'
import { localRefusal, parseToolResult, type ToolResult } from './tool-result'

/** An agent's hold: who, and the keys it covers (`@h:<HANDLE>` of an entity, a layer, a block or a layout). */
export type Hold = { pid: string; name: string; keys: string[] }

export interface EditSource {
  /** May this viewer change the file at all (a view-only room may not). */
  readonly canWrite: boolean
  /** Run one verb's argv (its words, then the kit's flags) on the file. */
  run(path: string, argv: readonly string[]): Promise<ToolResult>
  /** Agents' holds on the file now; empty when the server can't say. */
  holds(path: string): Promise<Hold[]>
}

/** The doc tool route: `{path, argv, json: true}` answers the kit's result object. */
export const TOOL_ROUTE = 'docs/tool'
/** Who's on the file, with each one's live claims. */
export const WHO_ROUTE = 'docs/who'

/** Refusals made here, never the kit's. */
export const NO_ROUTE = 'K2-NO-EDIT-ROUTE'
export const NOT_SENT = 'K2-NOT-SENT'

const MISSING = new Set([404, 405, 501])
const KIT_ID = /^(MPC|DXF|CAD)-E\d+$/

/** A thrown answer, as a refusal: the kit's id when the server named one. */
export function refusalOfError(e: unknown): ToolResult {
  const status = httpStatusOf(e)
  if (status !== null && MISSING.has(status)) return localRefusal(NO_ROUTE)
  const fromBody = e instanceof DaemonHttpError ? parseToolResult(e.body) : null
  if (fromBody) return fromBody
  const code = httpCodeOf(e)
  const id = code && KIT_ID.test(code) ? code : (/\b((?:MPC|DXF|CAD)-E\d+)\b/.exec(errorMessage(e))?.[1] ?? NOT_SENT)
  const details = e instanceof DaemonHttpError && e.details ? e.details : {}
  return localRefusal(id, errorMessage(e), details)
}

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)

/** Agents' claims from `docs/who`'s roster: each claim's node keys under its holder. */
export function parseHolds(body: unknown): Hold[] {
  const roster = isObj(body) && Array.isArray(body.roster) ? body.roster : []
  const out: Hold[] = []
  for (const e of roster) {
    if (!isObj(e) || e.kind !== 'agent' || typeof e.pid !== 'string') continue
    const keys = (Array.isArray(e.claims) ? e.claims : []).flatMap((c) =>
      isObj(c) && Array.isArray(c.nodes) ? c.nodes.filter((k): k is string => typeof k === 'string') : [],
    )
    if (keys.length) out.push({ pid: e.pid, name: typeof e.name === 'string' && e.name ? e.name : 'An agent', keys })
  }
  return out
}

export function daemonEditSource(scope: ServerScope): EditSource {
  return {
    canWrite: scopeMayWrite(scope, TOOL_ROUTE),
    async run(path, argv) {
      try {
        return parseToolResult(await daemonCliPost<unknown>(scope, TOOL_ROUTE, { path, argv: [...argv], json: true })) ?? localRefusal(NO_ROUTE)
      } catch (e) {
        return refusalOfError(e)
      }
    },
    async holds(path) {
      try {
        return parseHolds(await daemonCliGet<unknown>(scope, WHO_ROUTE, { path }))
      } catch {
        return []
      }
    },
  }
}

/** A source that answers from a function, for tests and shots; it records every call. */
export function fixtureEditSource(
  answer: (argv: readonly string[], n: number) => unknown,
  holds: Hold[] = [],
): EditSource & { calls: string[][]; setHolds(h: Hold[]): void } {
  const calls: string[][] = []
  let now = holds
  return {
    canWrite: true,
    calls,
    setHolds: (h) => void (now = h),
    async run(_path, argv) {
      calls.push([...argv])
      return parseToolResult(answer(argv, calls.length - 1)) ?? localRefusal(NO_ROUTE)
    },
    async holds() {
      return now
    },
  }
}

// The any-file (tier 0) routes on `/cli/docs/*`, for the file tab's panel:
// who's here, the lock, versions, conflict copies, approvals. The daemon owns
// all of it (`crates/k2-daemon/src/docs/tier0.rs`); these only call it, on the
// scope of the room the tab is in. Answers are read loudly: a shape this
// client doesn't know throws.

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

export interface Tier0Lease {
  pid: string
  name: string
  label: string
  expiresMs: number
  /** Taken by the kit around a save, not by a person. */
  auto: boolean
  mine: boolean
}

export interface Tier0ConflictCopy {
  /** Workspace-relative. */
  path: string
  /** The version whose content it holds. */
  kept: number
  atMs: number
}

export interface Tier0Status {
  version: number
  lease: Tier0Lease | null
  rights: { read: boolean; edit: boolean; history: boolean }
  /** Null without the history right: copies hold earlier content. */
  conflictCopies: Tier0ConflictCopy[] | null
}

export interface Tier0Person {
  pid: string
  name: string
  colour: string
  kind: string
  region: string | null
  away: boolean
}

export interface Tier0Version {
  id: number
  atMs: number
  name: string | null
  authorLabel: string
  size: number
  restoredFrom: number | null
  redacted: boolean
  current: boolean
}

export interface Tier0Approval {
  /** The version waiting for a decision. */
  change: number
  author: string
  onBehalfOf: string
}

export interface Tier0Approvals {
  pending: Tier0Approval[]
  /** The versions this caller may decide (made on their behalf). */
  mine: Set<number>
}

function obj(v: unknown, what: string): Record<string, unknown> {
  if (!v || typeof v !== 'object' || Array.isArray(v)) throw new Error(`docs: ${what} is not an object`)
  return v as Record<string, unknown>
}

function list(v: unknown, what: string): unknown[] {
  if (!Array.isArray(v)) throw new Error(`docs: ${what} is not a list`)
  return v
}

function num(v: unknown, what: string): number {
  if (typeof v !== 'number') throw new Error(`docs: ${what} is not a number`)
  return v
}

function str(v: unknown, what: string): string {
  if (typeof v !== 'string') throw new Error(`docs: ${what} is not text`)
  return v
}

const optStr = (v: unknown): string | null => (typeof v === 'string' && v !== '' ? v : null)
const optNum = (v: unknown): number | null => (typeof v === 'number' ? v : null)

function parseLease(v: unknown): Tier0Lease | null {
  if (v === null || v === undefined) return null
  const l = obj(v, 'lease')
  return {
    pid: str(l.pid, 'lease.pid'),
    name: str(l.name, 'lease.name'),
    label: str(l.label, 'lease.label'),
    expiresMs: num(l.expiresMs, 'lease.expiresMs'),
    auto: l.auto === true,
    mine: l.mine === true,
  }
}

export function parseTier0Status(body: unknown): Tier0Status {
  const b = obj(body, 'status')
  if (b.tier !== 0) throw new Error('docs: status is not for an any-file doc')
  const rights = obj(b.rights, 'rights')
  const copies = b.conflictCopies
  return {
    version: num(b.version, 'version'),
    lease: parseLease(b.lease),
    rights: { read: rights.read === true, edit: rights.edit === true, history: rights.history === true },
    conflictCopies:
      copies === null || copies === undefined
        ? null
        : list(copies, 'conflictCopies').map((c) => {
            const o = obj(c, 'conflict copy')
            return { path: str(o.path, 'copy.path'), kept: num(o.kept, 'copy.kept'), atMs: num(o.atMs, 'copy.atMs') }
          }),
  }
}

export function parseTier0Who(body: unknown): Tier0Person[] {
  return list(obj(body, 'who').roster, 'roster').map((e) => {
    const o = obj(e, 'roster entry')
    return {
      pid: str(o.pid, 'roster.pid'),
      name: str(o.name, 'roster.name'),
      colour: typeof o.colour === 'string' ? o.colour : '',
      kind: typeof o.kind === 'string' ? o.kind : 'person',
      region: optStr(o.region),
      away: o.away === true,
    }
  })
}

export function parseTier0Versions(body: unknown): Tier0Version[] {
  return list(obj(body, 'versions').versions, 'versions').map((e) => {
    const o = obj(e, 'version')
    return {
      id: num(o.id, 'version.id'),
      atMs: num(o.atMs, 'version.atMs'),
      name: optStr(o.name),
      authorLabel: str(o.authorLabel, 'version.authorLabel'),
      size: num(o.size, 'version.size'),
      restoredFrom: optNum(o.restoredFrom),
      redacted: o.redacted === true,
      current: o.current === true,
    }
  })
}

function parseApprovalRows(body: unknown): Tier0Approval[] {
  return list(obj(body, 'approvals').approvals, 'approvals').map((e) => {
    const o = obj(e, 'approval row')
    const a = obj(o.approval, 'approval')
    return { change: num(o.change, 'approval.change'), author: str(a.author, 'approval.author'), onBehalfOf: str(a.onBehalfOf, 'approval.onBehalfOf') }
  })
}

export const fetchTier0Status = async (scope: ServerScope, path: string): Promise<Tier0Status> =>
  parseTier0Status(await daemonCliGet<unknown>(scope, 'docs/status', { path }))

export const fetchTier0Who = async (scope: ServerScope, path: string): Promise<Tier0Person[]> =>
  parseTier0Who(await daemonCliGet<unknown>(scope, 'docs/who', { path }))

export const fetchTier0Versions = async (scope: ServerScope, path: string): Promise<Tier0Version[]> =>
  parseTier0Versions(await daemonCliGet<unknown>(scope, 'docs/versions', { path }))

/** Pending approvals, and which of them are this caller's to decide. */
export async function fetchTier0Approvals(scope: ServerScope, path: string): Promise<Tier0Approvals> {
  const [all, mine] = await Promise.all([
    daemonCliGet<unknown>(scope, 'docs/approvals', { path, pending: true }),
    daemonCliGet<unknown>(scope, 'docs/approvals', { path, pending: true, mine: true }),
  ])
  return { pending: parseApprovalRows(all), mine: new Set(parseApprovalRows(mine).map((a) => a.change)) }
}

export type Tier0LeaseAction = 'acquire' | 'renew' | 'release'

/** Take, renew or let go of the lock. The daemon's default term (5 minutes). */
export async function postTier0Lease(scope: ServerScope, path: string, action: Tier0LeaseAction): Promise<void> {
  await daemonCliPost<unknown>(scope, 'docs/lease', { path, action })
}

export interface Tier0RestoreResult {
  changed: boolean
  written: boolean
}

/** Make a version current again, as a new version: history is never rewritten. */
export async function postTier0Restore(scope: ServerScope, path: string, version: number): Promise<Tier0RestoreResult> {
  const b = obj(await daemonCliPost<unknown>(scope, 'docs/restore', { path, version }), 'restore')
  const r = obj(b.result, 'restore.result')
  return { changed: r.changed === true, written: r.written === true }
}

export interface Tier0Decision {
  /** On a decline: the version that undoes it (nothing is restored for you). */
  restoreTo: number | null
  /** Later versions that restoring it would also replace. */
  later: number[]
}

export async function postTier0Decision(scope: ServerScope, path: string, change: number, state: 'approved' | 'declined'): Promise<Tier0Decision> {
  const b = obj(await daemonCliPost<unknown>(scope, 'docs/approve', { path, change, state }), 'approve')
  return { restoreTo: optNum(b.restoreTo), later: Array.isArray(b.later) ? b.later.filter((v): v is number => typeof v === 'number') : [] }
}

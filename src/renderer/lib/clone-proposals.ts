// Agent clone proposals in the app (0.46.1; prd-clone-carries-threads-and-
// vault-v1 §4.4, CL21). An agent prepares a clone of its own workspace with
// `k2 clone propose`; a person (Owner/Admin) reviews it in the Clone dialog,
// prefilled from the proposal, and approves + runs it there.
//
// The daemon owns proposals (`/cli/clone-proposal/*`, table
// `clone_proposals`). This module is the thin adapter:
//
//   GET  clone-proposal/list?workspace=   open proposals for a workspace
//   POST clone/vault/confirm  {…ask, proposal, method, password?}
//        ONE person confirm → { token (clone-carry, null when nothing is
//        copied), approveToken (clone-approve for this proposal) }
//   POST clone-proposal/approve {proposal, confirmToken: approveToken}
//   POST clone-proposal/deny    {proposal, reason?}
//   POST clone-proposal/finish  {proposal, ok, detail?}   after the run
//
// Names and choices only; never a secret value.

import type { ServerScope } from '@/kessel/server-scope'
import { CloneConfirmCancelled, type AskPassword, type CloneCarryAsk, type SecretChoice } from './clone-options'

export interface CloneProposalOptions {
  threads: boolean
  sinceDays: number | null
  secrets: Record<string, SecretChoice>
  allHistory: boolean
  dest: string | null
}

export interface CloneProposal {
  id: string
  projectId: string
  proposedBy: string
  /** A host (`luzz.k2.dev`) or `here`. */
  target: string
  options: CloneProposalOptions
  why: string | null
  status: string
  expiresAt: number
  workspaceHandle: string | null
  workspacePath: string | null
  summary: string
}

type Get = <T = unknown>(scope: ServerScope, route: string, params?: Record<string, string>) => Promise<T>
type Post = <T = unknown>(scope: ServerScope, route: string, body?: unknown) => Promise<T>

const CHOICES: readonly SecretChoice[] = ['copy', 'blank', 'skip']

function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}

/** One proposal row from the daemon (null when it isn't one). */
export function parseProposal(raw: unknown): CloneProposal | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const id = str(r.id)
  const target = str(r.target)
  if (!id || !target) return null
  const o = (r.options && typeof r.options === 'object' ? r.options : {}) as Record<string, unknown>
  const secrets: Record<string, SecretChoice> = {}
  if (o.secrets && typeof o.secrets === 'object') {
    for (const [name, c] of Object.entries(o.secrets as Record<string, unknown>)) {
      if (CHOICES.includes(c as SecretChoice)) secrets[name] = c as SecretChoice
    }
  }
  return {
    id,
    projectId: str(r.projectId),
    proposedBy: str(r.proposedBy),
    target,
    options: {
      threads: o.threads !== false,
      sinceDays: typeof o.sinceDays === 'number' && o.sinceDays > 0 ? o.sinceDays : null,
      secrets,
      allHistory: o.allHistory !== false,
      dest: typeof o.dest === 'string' && o.dest ? o.dest : null,
    },
    why: typeof r.why === 'string' && r.why ? r.why : null,
    status: str(r.status),
    expiresAt: typeof r.expiresAt === 'number' ? r.expiresAt : 0,
    workspaceHandle: typeof r.workspaceHandle === 'string' ? r.workspaceHandle : null,
    workspacePath: typeof r.workspacePath === 'string' ? r.workspacePath : null,
    summary: str(r.summary),
  }
}

/** Open proposals for one workspace (handle, id or path). */
export async function fetchOpenProposals(get: Get, scope: ServerScope, workspace: string): Promise<CloneProposal[]> {
  const body = await get<{ proposals?: unknown }>(scope, 'clone-proposal/list', { status: 'open', workspace })
  const rows = Array.isArray(body?.proposals) ? body.proposals : []
  return rows.map(parseProposal).filter((p): p is CloneProposal => p !== null && p.status === 'open')
}

/** Who proposed it, in plain words: `agent:sales/s1` → `sales`. */
export function proposerName(p: Pick<CloneProposal, 'proposedBy'>): string {
  const m = /^agent:([^/]+)/.exec(p.proposedBy)
  return m ? m[1] : p.proposedBy || 'an agent'
}

/** The proposal's secret choices applied over the dialog's defaults, where
 *  the preview has the row and the choice is allowed (Copy only where the
 *  daemon allows it). */
export function applyProposalSecrets(
  initial: Record<string, SecretChoice>,
  proposal: Record<string, SecretChoice>,
  allowCopy: (name: string) => boolean,
): Record<string, SecretChoice> {
  const out = { ...initial }
  for (const [name, c] of Object.entries(proposal)) {
    if (!(name in out)) continue
    if (c === 'copy' && !allowCopy(name)) continue
    out[name] = c
  }
  return out
}

/** The answer of the one confirm. */
export interface ProposalConfirm {
  /** `clone-carry` for the plan; null when nothing is copied. */
  token: string | null
  /** `clone-approve` for the proposal. */
  approveToken: string
}

/**
 * ONE person confirm that approves the proposal and, when the plan copies
 * secrets or sends `.env` files, covers that too (the daemon mints both
 * tokens from one check and writes the Touch ID reason naming both). The
 * methods come from `vault/confirm/start`.
 */
export async function cloneProposalConfirm(
  post: Post,
  scope: ServerScope,
  projectPath: string,
  ask: CloneCarryAsk,
  proposalId: string,
  label: string,
  askPassword: AskPassword,
): Promise<ProposalConfirm> {
  const start = await post<{ methods?: unknown }>(scope, 'vault/confirm/start', { action: 'clone-approve' })
  const methods = Array.isArray(start?.methods) ? (start.methods as string[]) : []
  const finish = async (method: string, password?: string): Promise<ProposalConfirm> => {
    const r = await post<{ token?: unknown; approveToken?: unknown }>(scope, 'clone/vault/confirm', {
      project_path: projectPath,
      ...ask,
      proposal: proposalId,
      method,
      ...(password !== undefined ? { password } : {}),
    })
    if (typeof r?.approveToken !== 'string' || !r.approveToken) throw new Error('The confirm returned no approval.')
    return { token: typeof r.token === 'string' && r.token ? r.token : null, approveToken: r.approveToken }
  }
  if (methods.includes('local-auth')) return finish('local-auth')
  const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
  if (!pw) throw new Error('No way to confirm here: set a vault password first (Settings → Vault).')
  const password = await askPassword(pw, label)
  if (password === null) throw new CloneConfirmCancelled()
  return finish(pw, password)
}

export async function approveProposal(post: Post, scope: ServerScope, id: string, approveToken: string): Promise<void> {
  await post(scope, 'clone-proposal/approve', { proposal: id, confirmToken: approveToken })
}

export async function denyProposal(post: Post, scope: ServerScope, id: string, reason?: string): Promise<void> {
  await post(scope, 'clone-proposal/deny', { proposal: id, ...(reason ? { reason } : {}) })
}

export async function finishProposal(post: Post, scope: ServerScope, id: string, ok: boolean, detail?: string): Promise<void> {
  await post(scope, 'clone-proposal/finish', { proposal: id, ok, ...(detail ? { detail: detail.slice(0, 300) } : {}) })
}

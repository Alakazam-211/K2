// Clone dialog options: the names-only preview, the Thread tick, the
// per-secret choices and the person confirms (prd-clone-carries-threads-
// and-vault-v1 §4.1, D1, D3, D4).
//
// Pure helpers + thin daemon adapters, so the dialog store and the
// orchestration stay testable with mocks:
//
//   - `POST /cli/clone/preview` on the SOURCE daemon → `ClonePreview`.
//   - `initialSecretChoices` applies the daemon's per-row default, the
//     `copyable` flag and the Member rule (D1: never Copy).
//   - `buildCloneExtras` is the extra body the bundle/pack routes take
//     (`threads`, `secrets`, `confirm_token`, `env_confirm_token`,
//     `proposal_id`). Values never appear here: names and choices only.
//   - `getVaultConfirmToken` runs the existing vault confirm start/finish
//     flow (Touch ID on the Mac, else a password the dialog asks for).
//   - `cloneCarryConfirm` is the ONE seam for the vault slice's
//     `clone-carry` confirm (plan-bound); until that slice lands it throws
//     a clear message instead of cloning without it.

import type { ServerScope } from '@/kessel/server-scope'

export type SecretChoice = 'copy' | 'blank' | 'skip'

export interface ClonePreviewSecretRow {
  name: string
  kind: string
  group: 'workspace' | 'server'
  default: SecretChoice
  copyable: boolean
  note?: string
}

export interface ClonePreview {
  ok?: boolean
  workspace?: { id: string; handle: string; path: string }
  history?: { sessions: number; files: number; entries: number }
  threads: {
    available: boolean
    conversations?: number
    docs?: number
    participants?: string[]
  }
  secrets: {
    available: boolean
    rows: ClonePreviewSecretRow[]
    notCarried: string[]
  }
  envFiles: string[]
  callerCanCopySecrets: boolean
}

/** `threads` as the bundle/pack routes take it. */
export type CloneThreads = 'all' | 'none' | { sinceDays: number }

/** The extra fields this slice adds to `clone/bundle` / `clone/pack`. */
export interface CloneExtras {
  threads: CloneThreads
  secrets?: Record<string, SecretChoice>
  confirm_token?: string
  env_confirm_token?: string
  proposal_id?: string
}

/** Since choices offered under the Thread tick (null = all). */
export const THREAD_SINCE_OPTIONS: { value: number | null; label: string }[] = [
  { value: null, label: 'all' },
  { value: 30, label: '30 days' },
  { value: 90, label: '90 days' },
]

/** Copy needs an Owner/Admin (D1). */
export const MEMBER_NO_COPY_LINE = 'Copying secrets needs an Owner or Admin'

/** Shown when a Copy row is chosen before the vault update is installed. */
export const CARRY_NOT_READY =
  'Copying secrets arrives with the vault update. Choose Leave blank or Skip for now.'

/** Normalize whatever the daemon sent (older daemons have no preview). */
export function normalizePreview(raw: unknown): ClonePreview {
  const r = (raw ?? {}) as Partial<ClonePreview>
  const threads = r.threads && typeof r.threads === 'object' ? r.threads : { available: false }
  const secrets = r.secrets && typeof r.secrets === 'object' ? r.secrets : { available: false, rows: [], notCarried: [] }
  return {
    ...r,
    threads: { ...threads, available: threads.available === true },
    secrets: {
      available: secrets.available === true,
      rows: Array.isArray(secrets.rows) ? secrets.rows : [],
      notCarried: Array.isArray(secrets.notCarried) ? secrets.notCarried : [],
    },
    envFiles: Array.isArray(r.envFiles) ? r.envFiles : [],
    callerCanCopySecrets: r.callerCanCopySecrets === true,
  }
}

/** Whether Copy may be picked for a row. */
export function canCopyRow(row: ClonePreviewSecretRow, callerCanCopy: boolean): boolean {
  return callerCanCopy && row.copyable
}

/** Per-row starting choice: the daemon default, never Copy where Copy
 *  isn't allowed (a Member, or a row that can't be copied → Leave blank). */
export function initialSecretChoices(preview: ClonePreview): Record<string, SecretChoice> {
  const out: Record<string, SecretChoice> = {}
  for (const row of preview.secrets.rows) {
    const d: SecretChoice = row.default === 'copy' || row.default === 'blank' || row.default === 'skip' ? row.default : 'skip'
    out[row.name] = d === 'copy' && !canCopyRow(row, preview.callerCanCopySecrets) ? 'blank' : d
  }
  return out
}

export function anyCopy(choices: Record<string, SecretChoice>): boolean {
  return Object.values(choices).some((c) => c === 'copy')
}

export function threadsValue(bring: boolean, sinceDays: number | null): CloneThreads {
  if (!bring) return 'none'
  return sinceDays ? { sinceDays } : 'all'
}

/** The bundle/pack extras for these choices (no tokens yet). */
export function buildCloneExtras(args: {
  bringThread: boolean
  sinceDays: number | null
  secretChoices: Record<string, SecretChoice>
  proposalId?: string | null
}): CloneExtras {
  const extras: CloneExtras = { threads: threadsValue(args.bringThread, args.sinceDays) }
  if (Object.keys(args.secretChoices).length > 0) extras.secrets = { ...args.secretChoices }
  if (args.proposalId) extras.proposal_id = args.proposalId
  return extras
}

/** `secretId` the daemon binds the `clone-env` confirm to (D4). */
export function envConfirmSecretId(projectPath: string): string {
  return `env-files:${projectPath}`
}

/** Plain-words participants line (owner already shown as "you" by the daemon). */
export function threadSummaryLine(t: ClonePreview['threads']): string | null {
  if (!t.available) return null
  const parts: string[] = []
  if (typeof t.conversations === 'number') parts.push(`${t.conversations} conversation${t.conversations === 1 ? '' : 's'}`)
  if (typeof t.docs === 'number') parts.push(`${t.docs} message${t.docs === 1 ? '' : 's'}`)
  return parts.length > 0 ? parts.join(', ') : null
}

type Post = <T = unknown>(scope: ServerScope, route: string, body?: unknown) => Promise<T>

/** `POST /cli/clone/preview` on the source daemon. */
export async function fetchClonePreview(
  post: Post,
  scope: ServerScope,
  projectPath: string,
): Promise<ClonePreview> {
  return normalizePreview(await post(scope, 'clone/preview', { project_path: projectPath }))
}

/** Asks the person for a password (the dialog renders the field). Resolves
 *  null when they cancel. */
export type AskPassword = (method: string, label: string) => Promise<string | null>

export class CloneConfirmCancelled extends Error {
  constructor() {
    super('Confirm cancelled.')
    this.name = 'CloneConfirmCancelled'
  }
}

/** The existing vault confirm flow for `action` bound to `secretId`. */
export async function getVaultConfirmToken(
  post: Post,
  scope: ServerScope,
  action: string,
  secretId: string,
  label: string,
  askPassword: AskPassword,
): Promise<string> {
  const { methods } = await post<{ methods: string[] }>(scope, 'vault/confirm/start', { action, secretId })
  const list = Array.isArray(methods) ? methods : []
  const finish = async (method: string, password?: string): Promise<string> => {
    const r = await post<{ token: string }>(scope, 'vault/confirm/finish', {
      action,
      secretId,
      method,
      label: label.slice(0, 64),
      ...(password !== undefined ? { password } : {}),
    })
    if (!r?.token) throw new Error('The confirm returned no token.')
    return r.token
  }
  if (list.includes('local-auth')) return finish('local-auth')
  const pw = list.find((m) => m === 'connect-password' || m === 'vault-password')
  if (!pw) {
    throw new Error('No way to confirm here: set a vault password first (Settings → Vault).')
  }
  const password = await askPassword(pw, label)
  if (password === null) throw new CloneConfirmCancelled()
  return finish(pw, password)
}

/**
 * The vault slice's plan-bound `clone-carry` confirm. ONE seam: the vault
 * slice wires the real plan digest here. Until then, a Copy row stops the
 * clone with a clear message (never clones without the confirm).
 */
export async function cloneCarryConfirm(_args: {
  post: Post
  scope: ServerScope
  projectPath: string
  secrets: Record<string, SecretChoice>
  askPassword: AskPassword
}): Promise<string> {
  throw new Error(CARRY_NOT_READY)
}

// Clone dialog options: the names-only preview, the Thread tick, the
// per-secret choices and the person confirm (prd-clone-carries-threads-
// and-vault-v1 §4.1, D1, D3, D4; slice A's vault plan, slice B's Threads).
//
// Pure helpers + thin daemon adapters, so the dialog store and the
// orchestration stay testable with mocks:
//
//   - `POST /cli/clone/preview` on the SOURCE daemon → `ClonePreview`. The
//     body carries slice A's `CarryAsk` (destination vault, clone id,
//     choices), so the Secrets block shows the real per-secret plan.
//   - `initialSecretChoices` takes each row's planned choice, never Copy
//     where Copy isn't allowed (a Member, D1, or a row that can't travel).
//   - `buildCloneExtras` is the non-vault part of the bundle/pack body
//     (`threads`, `clone_id`, `proposal_id`). Choices and the confirm token
//     ride slice A's `vaultBundleFields` (clone-vault.ts).
//   - `cloneVaultConfirm` is the ONE person confirm (`clone-carry`, bound to
//     the final plan's digest): it covers Copy rows and sending `.env` files
//     as files (D4). Touch ID when the source offers it, else a password.
//     Values never appear here: names and choices only.

import type { ServerScope } from '@/kessel/server-scope'
import type { CloneVaultTarget } from './clone-vault'

export type SecretChoice = 'copy' | 'blank' | 'skip'

export interface ClonePreviewSecretRow {
  name: string
  kind: string
  group: 'workspace' | 'server' | 'shared'
  default: SecretChoice
  /** The planned choice for the ask that was sent (no target → skip). */
  choice?: SecretChoice
  copyable: boolean
  note?: string
  envNames?: string[]
}

/** The `clone-carry` binding the source answers for a plan that needs one. */
export interface ClonePreviewConfirm {
  action: string
  secretId: string
  reason: string
  methods: string[]
  allowed: boolean
}

export interface ClonePreview {
  ok?: boolean
  workspace?: { id: string; handle: string; path: string }
  history?: { sessions: number; files: number; entries: number }
  threads: {
    available: boolean
    conversations?: number
    docs?: number
    dropped?: number
    participants?: string[]
    roles?: Record<string, number>
  }
  secrets: {
    available: boolean
    rows: ClonePreviewSecretRow[]
    notCarried: string[]
    member?: boolean
    needsConfirm?: boolean
    confirm?: ClonePreviewConfirm | null
    envFilesHint?: string | null
  }
  envFiles: string[]
  callerCanCopySecrets: boolean
}

/** Slice A's `CarryAsk` (snake_case wire), minus the source path. */
export interface CloneCarryAsk {
  vault_target?: CloneVaultTarget
  clone_id: string
  secrets?: Record<string, SecretChoice>
  own_box_move?: boolean
  send_env_files?: boolean
}

/** `threads` as the bundle/pack routes take it. */
export type CloneThreads = 'all' | 'none' | { sinceDays: number }

/** The non-vault fields this slice adds to `clone/bundle` / `clone/pack`.
 *  `confirm_token` is set here only for `.env` files with no destination
 *  vault (otherwise it rides the vault fields). */
export interface CloneExtras {
  threads: CloneThreads
  clone_id?: string
  proposal_id?: string
  confirm_token?: string
}

/** Since choices offered under the Thread tick (null = all). */
export const THREAD_SINCE_OPTIONS: { value: number | null; label: string }[] = [
  { value: null, label: 'all' },
  { value: 30, label: '30 days' },
  { value: 90, label: '90 days' },
]

/** Copy needs an Owner/Admin (D1). */
export const MEMBER_NO_COPY_LINE = 'Copying secrets needs an Owner or Admin'

const CHOICES: SecretChoice[] = ['copy', 'blank', 'skip']
const isChoice = (c: unknown): c is SecretChoice => typeof c === 'string' && (CHOICES as string[]).includes(c)

/** Normalize whatever the daemon sent (older daemons have no preview). */
export function normalizePreview(raw: unknown): ClonePreview {
  const r = (raw ?? {}) as Partial<ClonePreview>
  const threads = r.threads && typeof r.threads === 'object' ? r.threads : { available: false }
  const secrets = (r.secrets && typeof r.secrets === 'object' ? r.secrets : { available: false }) as Partial<ClonePreview['secrets']>
  return {
    ...r,
    threads: { ...threads, available: threads.available === true },
    secrets: {
      ...secrets,
      available: secrets.available === true,
      rows: Array.isArray(secrets.rows) ? secrets.rows : [],
      notCarried: Array.isArray(secrets.notCarried) ? secrets.notCarried : [],
      confirm: secrets.confirm ?? null,
    },
    envFiles: Array.isArray(r.envFiles) ? r.envFiles : [],
    callerCanCopySecrets: r.callerCanCopySecrets === true,
  }
}

/** Whether Copy may be picked for a row. */
export function canCopyRow(row: ClonePreviewSecretRow, callerCanCopy: boolean): boolean {
  return callerCanCopy && row.copyable
}

/** Per-row starting choice: the plan's choice (else the default), never
 *  Copy where Copy isn't allowed (a Member, or a row that can't travel →
 *  Leave blank). */
export function initialSecretChoices(preview: ClonePreview): Record<string, SecretChoice> {
  const out: Record<string, SecretChoice> = {}
  for (const row of preview.secrets.rows) {
    const d: SecretChoice = isChoice(row.choice) ? row.choice : isChoice(row.default) ? row.default : 'skip'
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

/** The non-vault extras for these choices (no tokens). */
export function buildCloneExtras(args: {
  bringThread: boolean
  sinceDays: number | null
  cloneId?: string | null
  proposalId?: string | null
}): CloneExtras {
  const extras: CloneExtras = { threads: threadsValue(args.bringThread, args.sinceDays) }
  if (args.cloneId) extras.clone_id = args.cloneId
  if (args.proposalId) extras.proposal_id = args.proposalId
  return extras
}

/** Slice A's ask for the final choices (what the confirm is minted for and
 *  what bundle/pack check it against). */
export function buildCarryAsk(args: {
  target: CloneVaultTarget | null
  cloneId: string
  secretChoices: Record<string, SecretChoice>
  ownBoxMove: boolean
  sendEnvFiles: boolean
}): CloneCarryAsk {
  const ask: CloneCarryAsk = { clone_id: args.cloneId }
  if (args.target) ask.vault_target = args.target
  if (Object.keys(args.secretChoices).length > 0) ask.secrets = { ...args.secretChoices }
  if (args.ownBoxMove) ask.own_box_move = true
  if (args.sendEnvFiles) ask.send_env_files = true
  return ask
}

/** Copy rows or `.env` files need the person confirm (§6.4, D4). */
export function needsCarryConfirm(choices: Record<string, SecretChoice>, sendEnvFiles: boolean): boolean {
  return sendEnvFiles || anyCopy(choices)
}

/** Plain-words counts line for the Thread tick. */
export function threadSummaryLine(t: ClonePreview['threads']): string | null {
  if (!t.available) return null
  const parts: string[] = []
  const roles = t.roles ?? {}
  const sidecars = roles.sidecar ?? 0
  if (typeof t.conversations === 'number') {
    if (sidecars > 0 && (roles.canonical ?? 0) > 0) {
      parts.push(`1 main Chat, ${sidecars} sidecar${sidecars === 1 ? '' : 's'}`)
    } else {
      parts.push(`${t.conversations} conversation${t.conversations === 1 ? '' : 's'}`)
    }
  }
  if (typeof t.docs === 'number') parts.push(`${t.docs} message${t.docs === 1 ? '' : 's'}`)
  return parts.length > 0 ? parts.join(', ') : null
}

type Post = <T = unknown>(scope: ServerScope, route: string, body?: unknown) => Promise<T>

/** `POST /cli/clone/preview` on the source daemon, with slice A's ask. */
export async function fetchClonePreview(
  post: Post,
  scope: ServerScope,
  projectPath: string,
  ask?: CloneCarryAsk,
  sinceDays?: number | null,
): Promise<ClonePreview> {
  return normalizePreview(
    await post(scope, 'clone/preview', {
      project_path: projectPath,
      ...(sinceDays ? { sinceDays } : {}),
      ...(ask ?? {}),
    }),
  )
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

/**
 * The `clone-carry` confirm for the FINAL ask (slice A,
 * `POST /cli/clone/vault/confirm` on the source). The daemon builds the
 * reason from the plan and binds the token to its digest (choices, target
 * and label, clone id, `.env` files), so any change after this needs a new
 * confirm. `methods` come from the preview for this ask.
 */
export async function cloneVaultConfirm(
  post: Post,
  scope: ServerScope,
  projectPath: string,
  ask: CloneCarryAsk,
  methods: string[],
  label: string,
  askPassword: AskPassword,
): Promise<string> {
  const finish = async (method: string, password?: string): Promise<string> => {
    const r = await post<{ token: string }>(scope, 'clone/vault/confirm', {
      project_path: projectPath,
      ...ask,
      method,
      ...(password !== undefined ? { password } : {}),
    })
    if (!r?.token) throw new Error('The confirm returned no token.')
    return r.token
  }
  if (methods.includes('local-auth')) return finish('local-auth')
  const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
  if (!pw) {
    throw new Error('No way to confirm here: set a vault password first (Settings → Vault).')
  }
  const password = await askPassword(pw, label)
  if (password === null) throw new CloneConfirmCancelled()
  return finish(pw, password)
}

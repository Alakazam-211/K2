// Agent templates (prd-agent-templates-v1 §3, §6.1, §7; the T5–T7 contract):
// the typed client for the Create an agent sheet and Save as template.
//
//   GET  /cli/agent-templates            the picker (built-ins, then the library)
//   GET  /cli/agent-templates/show?id=   one template: params, questions, tool
//   GET  /cli/agents/hire-root           where a new agent's folder goes
//   POST /cli/agents/hire                {template, params, answers, dryRun}
//   POST /cli/agent-templates/save       {workspace, id, name, summary, wiki, dryRun}
//
// The daemon decides everything (who may, what's written); the dry run is
// its own truth for each Review step. An older server answers the template
// routes with a plain 404 (no `code`): that becomes `TemplatesUnsupportedError`
// so a sheet can say so instead of showing a raw "route not found".

import { daemonCliGet, daemonCliPost, daemonCliPostQuery } from '@/lib/daemon-cli'
import { httpCodeOf, httpDetailsOf, httpStatusOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import { PRODUCT_NAME } from '@/lib/brand'

// ── Types (the daemon's JSON; camelCase from serde) ──────────────────

export type TemplateKind = 'agent' | 'system'
/** `builtin` (ships in K2) or `library` (`~/.k2/agent-templates`). */
export type TemplateOrigin = 'builtin' | 'library'

/** One `GET /cli/agent-templates` card. */
export interface TemplateSummary {
  id: string
  version: number
  name: string
  summary: string
  kind: TemplateKind | string
  aliases: string[]
  /** `K2` for built-ins; `user:<login>` or `agent:<handle>` otherwise. */
  author: string
  origin: TemplateOrigin | string
  hash: string
}

export interface TemplateList {
  ok: true
  /** The server has the save route (a static flag, not per caller: a caller
   *  who may not save still gets 403 from it). Missing on an older server. */
  canSave?: boolean
  templates: TemplateSummary[]
  /** Library folders the loader refused (shown nowhere yet). */
  problems: unknown[]
}

export interface TemplateParam {
  label: string
  max: number
  required: boolean
}

export interface TemplateQuestion {
  id: string
  ask: string
  /** Where an answer goes: `role` or a starter note. */
  fills: string
}

/** One `GET /cli/agent-templates/show` template. */
export interface TemplateDetail extends TemplateSummary {
  sharedBlock: boolean
  agent: { role: string; tool: string; model: string | null }
  catalog: string[]
  permissions: { cards: boolean; suggest: string[]; expanded: string[] }
  params: Record<string, TemplateParam>
  wiki: string[]
  layers: string[]
  skills: string[]
  heartbeats: Array<{ name: string; frequency: string }>
  hello: string | null
  /** Absent on a server from before `[[questions]]`. */
  questions?: TemplateQuestion[]
  dir: string
}

/** One row of the hire plan (dry run) or report. */
export interface HireRow {
  /** `folder`, `register`, `role`, `wiki`, `wiki-index`, `catalog`, `layer`,
   *  `skill`, `heartbeat`, `permission`, `lock`, `hello`. */
  step: string
  target: string
  /** `write` (dry run), `done`, `skip`, `yours`, `suggest`; T3 adds `grant`
   *  and `card`. */
  status: string
  reason?: string
}

export interface HireTemplateRef {
  id: string
  version: number
  hash: string
  kind: string
  name: string
  origin: string
  author: string
}

export interface HireQuestionState {
  id: string
  ask: string
  fills: string
  answered: boolean
}

/** `POST /cli/agents/hire` with a template: the plan (dry run) or the report. */
export interface HireResult {
  ok: true
  path: string
  /** The workspace id (empty in a dry run of a new folder). */
  id: string
  workspaceId: string
  handle: string
  name: string
  created: boolean
  changed: boolean
  dryRun: boolean
  template: HireTemplateRef
  steps: Array<{ step: string; status: string; reason?: string }>
  rows: HireRow[]
  /** The rendered charter: the Review's preview. */
  charter: string
  permissions: { cards: boolean; suggested: string[]; granted: string[]; carded: string[] }
  /** The first message the agent gets, on the first create only. */
  hello: string | null
  /** The daemon started the agent and delivered `hello` itself. Missing on
   *  an older daemon, which leaves the hello to the caller. */
  helloDelivered?: boolean
  questions: HireQuestionState[]
}

export interface HireRequest {
  template: string
  name: string
  /** The folder-name seed; the daemon uses it when `path` is left out. */
  handle?: string
  /** The folder. Left out: `<hire root>/<handle>`. */
  path?: string
  /** A preset id. */
  preset?: string
  params?: Record<string, string>
  answers?: Record<string, string>
  dryRun: boolean
  /** Start the agent and deliver its hello (the real create only; absent:
   *  no launch). */
  launch?: boolean
}

export interface HireRoot {
  ok: true
  root: string
  /** True while the server uses its built-in default. */
  default: boolean
}

/** `POST /cli/agent-templates/save` (the contract's T6). */
export interface SaveTemplateRequest {
  /** The workspace's path or handle. */
  workspace: string
  id: string
  name?: string
  summary?: string
  /** Wiki notes to take, by their path under `.k2/wiki/`. */
  wiki: string[]
  dryRun: boolean
}

export interface SaveSkipped {
  what: string
  name: string
  why: string
}

export interface SaveTemplateResult {
  ok: true
  dryRun: boolean
  id: string
  version: number
  author: string
  path: string
  hash: string
  files: Array<{ path: string; bytes: number }>
  catalog: string[]
  suggest: string[]
  tool: string | null
  heartbeats: string[]
  skipped: SaveSkipped[]
}

// ── Errors ───────────────────────────────────────────────────────────

/** The server has no such template route (a version from before it). */
export class TemplatesUnsupportedError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'TemplatesUnsupportedError'
  }
}

/** A 404 with no refusal code (the daemon's "route not found"), or the
 *  pre-0.44.4 method guard: the route isn't on this server. A 404 that
 *  carries a code (`no_workspace`, `not_found`) is a real answer. */
export function isMissingRoute(e: unknown): boolean {
  const status = httpStatusOf(e)
  if (status === 404) return httpCodeOf(e) === null
  if (status === 405) return /method not allowed for this route/i.test(e instanceof Error ? e.message : '')
  return false
}

async function templateCall<T>(call: () => Promise<T>, unsupported: string): Promise<T> {
  try {
    return await call()
  } catch (e) {
    if (isMissingRoute(e)) throw new TemplatesUnsupportedError(unsupported)
    throw e
  }
}

export const CREATE_UNSUPPORTED_TEXT = 'This server can’t create agents from templates yet.'
export const SAVE_UNSUPPORTED_TEXT = 'This server can’t save templates yet.'

// ── Calls ────────────────────────────────────────────────────────────

export function listTemplates(scope: ServerScope): Promise<TemplateList> {
  return templateCall(() => daemonCliGet<TemplateList>(scope, 'agent-templates'), CREATE_UNSUPPORTED_TEXT)
}

export async function showTemplate(scope: ServerScope, id: string): Promise<TemplateDetail> {
  const r = await templateCall(
    () => daemonCliGet<{ ok: true; template: TemplateDetail }>(scope, 'agent-templates/show', { id }),
    CREATE_UNSUPPORTED_TEXT,
  )
  return r.template
}

export function getHireRoot(scope: ServerScope): Promise<HireRoot> {
  return daemonCliGet<HireRoot>(scope, 'agents/hire-root')
}

export function hireFromTemplate(scope: ServerScope, req: HireRequest): Promise<HireResult> {
  return daemonCliPost<HireResult>(scope, 'agents/hire', req)
}

export function saveTemplate(scope: ServerScope, req: SaveTemplateRequest): Promise<SaveTemplateResult> {
  return templateCall(() => daemonCliPost<SaveTemplateResult>(scope, 'agent-templates/save', req), SAVE_UNSUPPORTED_TEXT)
}

/** After a create on an older daemon (no `helloDelivered`): start the agent,
 *  then give it the template's hello as its first message (what
 *  `k2 agent hire --launch` does). A newer daemon delivers it itself. */
export async function sayHello(scope: ServerScope, path: string, hello: string): Promise<void> {
  await daemonCliPostQuery(scope, 'workspace/ensure-canonical-session', { project: path })
  await daemonCliPostQuery(scope, 'workspace/msg', {
    workspace: path,
    text: hello,
    from: 'owner',
    wake: 'true',
    wake_timeout: '120',
  })
}

// ── Picking ──────────────────────────────────────────────────────────

/** The picker's groups: built-ins (General first) and "Yours" (the
 *  library). System templates (K5, the Chief of Staff) are never shown. */
export function pickerGroups(list: readonly TemplateSummary[]): { builtIn: TemplateSummary[]; yours: TemplateSummary[] } {
  const shown = list.filter((t) => t.kind !== 'system')
  const builtIn = shown
    .filter((t) => t.origin !== 'library')
    .sort((a, b) => (a.id === 'general' ? -1 : b.id === 'general' ? 1 : 0))
  const yours = shown.filter((t) => t.origin === 'library')
  return { builtIn, yours }
}

/** "user:rosson" → "rosson", "agent:scout" → "the agent scout". */
export function authorLabel(author: string): string {
  const a = author.trim()
  if (a.startsWith('user:')) return a.slice(5)
  if (a.startsWith('agent:')) return `the agent ${a.slice(6)}`
  return a
}

// ── Review rows ──────────────────────────────────────────────────────

/** What each plan step is, in the Review's words. */
export const HIRE_STEP_LABELS: Record<string, string> = {
  folder: 'Folder',
  register: `Add to ${PRODUCT_NAME}`,
  role: 'Charter',
  wiki: 'Wiki note',
  'wiki-index': 'Wiki index',
  catalog: 'Context',
  layer: 'Context layer',
  skill: 'Skill',
  heartbeat: 'Heartbeat',
  permission: 'Permission',
  lock: 'Template record',
  hello: 'First message',
}

/** What each row's status means. */
export const HIRE_STATUS_LABELS: Record<string, string> = {
  write: 'Will add',
  done: 'Added',
  skip: 'Already there',
  yours: 'Kept as it is',
  suggest: 'Suggested',
  grant: 'Will be granted',
  card: 'Asks you on a card',
}

export function hireStepLabel(step: string): string {
  return HIRE_STEP_LABELS[step] ?? step
}

export function hireStatusLabel(status: string): string {
  return HIRE_STATUS_LABELS[status] ?? status
}

/** The permission row's target in words. */
export function permissionTarget(target: string): string {
  return target === 'permission-cards' ? 'Can ask for permission cards' : target
}

// ── Save errors ──────────────────────────────────────────────────────

export const SAVE_ERROR_COPY = {
  bad_id: 'Use lowercase letters, numbers and single hyphens for the id.',
  id_reserved: `That id belongs to a template built into ${PRODUCT_NAME}. Choose another id.`,
  id_taken: 'Someone else already saved a template with that id. Choose another id.',
  own_workspace_only: 'An agent can save only its own setup as a template.',
  too_large: 'This setup is too big for a template. Take fewer wiki notes, or make its files smaller.',
  no_workspace: `${PRODUCT_NAME} can’t find this agent on the server anymore.`,
  forbidden: 'Only an Owner or Admin can save templates.',
  unsupported: SAVE_UNSUPPORTED_TEXT,
} as const

/** The secret check's refusal: the file and line, never the secret. */
export function secretFoundText(file: string | null, line: number | null): string {
  const where = file ? (line ? `${file}, line ${line}` : file) : null
  return where
    ? `${where} has something that looks like a secret. Remove it, then save again.`
    : 'A file has something that looks like a secret. Remove it, then save again.'
}

export interface SaveErrorView {
  code: string | null
  /** What went wrong, in the sheet's words. */
  text: string
  /** The server's own words, shown quietly; null when they mustn't be
   *  shown (a secret) or the sheet's text says it all. */
  detail: string | null
  /** The fix is in the fields (the id): the sheet goes back to them. */
  fieldsProblem: boolean
}

function num(v: unknown): number | null {
  if (typeof v === 'number' && Number.isInteger(v) && v > 0) return v
  if (typeof v === 'string' && /^\d+$/.test(v)) return Number(v)
  return null
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

/** A save refusal as the sheet says it, by code. */
export function saveErrorView(e: unknown): SaveErrorView {
  if (e instanceof TemplatesUnsupportedError) {
    return { code: 'unsupported', text: SAVE_ERROR_COPY.unsupported, detail: null, fieldsProblem: false }
  }
  const code = httpCodeOf(e)
  const message = (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
  if (code === 'secret_found') {
    // The file and line ride the error object (or the body's top level).
    const d = httpDetailsOf(e) ?? {}
    const body = (e as { body?: unknown } | null)?.body
    const top = body && typeof body === 'object' ? (body as Record<string, unknown>) : {}
    const file = str(d.file) ?? str(top.file)
    const line = num(d.line) ?? num(top.line)
    return { code, text: secretFoundText(file, line), detail: null, fieldsProblem: false }
  }
  if (code && code in SAVE_ERROR_COPY) {
    const text = SAVE_ERROR_COPY[code as keyof typeof SAVE_ERROR_COPY]
    const fieldsProblem = code === 'bad_id' || code === 'id_reserved' || code === 'id_taken'
    return { code, text, detail: code === 'too_large' ? message : null, fieldsProblem }
  }
  if (httpStatusOf(e) === 403) {
    return { code, text: SAVE_ERROR_COPY.forbidden, detail: message, fieldsProblem: false }
  }
  return { code, text: `${PRODUCT_NAME} couldn’t save the template.`, detail: message, fieldsProblem: false }
}

// ── Create errors ────────────────────────────────────────────────────

export interface HireErrorView {
  text: string
  detail: string
  /** The fix is the name or folder: the sheet offers going back to them. */
  folderProblem: boolean
}

/** A hire refusal as the sheet says it. */
export function hireErrorView(e: unknown): HireErrorView {
  const code = httpCodeOf(e)
  const detail = (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
  if (e instanceof TemplatesUnsupportedError || isMissingRoute(e)) {
    return { text: CREATE_UNSUPPORTED_TEXT, detail, folderProblem: false }
  }
  if (code === 'folder_exists') {
    return { text: 'That folder is already in use. Choose another name or folder.', detail, folderProblem: true }
  }
  if (code === 'template_conflict') {
    return { text: 'That folder already holds an agent made from another template. Choose another folder.', detail, folderProblem: true }
  }
  if (/permission denied|EACCES|EPERM|not writable|read-only file system/i.test(detail)) {
    return { text: `${PRODUCT_NAME} can’t write to that folder. Choose another folder.`, detail, folderProblem: true }
  }
  return { text: `${PRODUCT_NAME} couldn’t make the agent.`, detail, folderProblem: false }
}

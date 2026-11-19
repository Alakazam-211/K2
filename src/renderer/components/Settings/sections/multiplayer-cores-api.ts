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

/** The docs store's saves (`saves` on `GET /cli/docs/cores`, kit v0.23.0). */
export interface DocSaves {
  /** The owner's "Save every change immediately" (`docsSaveImmediately`). */
  saveEveryChangeImmediately: boolean
  /** Only the server's owner changes it. */
  canChange: boolean
  /** What the docs store runs with now; null while live docs are off. */
  running: 'grouped' | 'immediate' | null
  /** The setting takes effect after K2 restarts, and it hasn't yet. */
  restartNeeded: boolean
  /** The server's OS (`macos`, `linux`, `windows`); null from a server that doesn't say. */
  os: string | null
}

export interface DocCores {
  canChange: boolean
  /** `.k5board` files open in a Board tab on this server (the preview switch);
   *  null on a server from before Board files, which has no such switch. */
  boardFilePreview: boolean | null
  /** "Save every change immediately" (kit v0.23.0); null on a server from before it. */
  saves: DocSaves | null
  /** Cores still to come, plain words ("CSV grid", "Word"). */
  later: string[]
  cores: DocCore[]
}

/** Does the window's server have the multiplayer switches? */
export function multiplayerCoresAvailable(scope: ServerScope = primaryScope()): boolean {
  return scope.serverSupports('docs-cores-v1')
}

// ── Parsing (loud: a shape we don't know is an error, never a guess) ──
// The messages are the quiet second line of the page's error card, so they
// say what was wrong with the reply in words, without the route's name.

/** `the reply's canChange`, `cores[0].name`. */
function field(what: string, key: string): string {
  return what === 'the reply' ? `the reply's ${key}` : `${what}.${key}`
}

function obj(v: unknown, what: string): Record<string, unknown> {
  if (v === null || typeof v !== 'object' || Array.isArray(v)) throw new Error(`${what} is not an object`)
  return v as Record<string, unknown>
}

function str(o: Record<string, unknown>, key: string, what: string): string {
  const v = o[key]
  if (typeof v !== 'string') throw new Error(`${field(what, key)} is not text`)
  return v
}

function num(o: Record<string, unknown>, key: string, what: string): number {
  const v = o[key]
  if (typeof v !== 'number' || !Number.isFinite(v)) throw new Error(`${field(what, key)} is not a number`)
  return v
}

function bool(o: Record<string, unknown>, key: string, what: string): boolean {
  const v = o[key]
  if (typeof v !== 'boolean') throw new Error(`${field(what, key)} is not true or false`)
  return v
}

function strings(v: unknown, what: string): string[] {
  if (!Array.isArray(v) || v.some((x) => typeof x !== 'string')) throw new Error(`${what} is not a list of words`)
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
  if (!Array.isArray(o.failed)) throw new Error(`${w}.failed is not a list`)
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

function parseSaves(v: unknown): DocSaves {
  const w = "the reply's saves"
  const o = obj(v, w)
  const running = o.running
  if (running !== null && running !== 'grouped' && running !== 'immediate') throw new Error(`${w}.running is not grouped, immediate or null`)
  return {
    saveEveryChangeImmediately: bool(o, 'saveEveryChangeImmediately', w),
    canChange: bool(o, 'canChange', w),
    running,
    restartNeeded: bool(o, 'restartNeeded', w),
    os: o.os === undefined || o.os === null ? null : str(o, 'os', w),
  }
}

/** `GET /cli/docs/cores`'s body. Throws on any field it can't read. */
export function parseDocCores(body: unknown): DocCores {
  const o = obj(body, 'the reply')
  if (!Array.isArray(o.cores)) throw new Error("the server's answer was missing the list of file kinds")
  return {
    canChange: bool(o, 'canChange', 'the reply'),
    boardFilePreview: o.boardFilePreview === undefined || o.boardFilePreview === null ? null : bool(o, 'boardFilePreview', 'the reply'),
    saves: o.saves === undefined || o.saves === null ? null : parseSaves(o.saves),
    later: o.later === undefined || o.later === null ? [] : strings(o.later, "the reply's later"),
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

/** "Save every change immediately" on or off: `{"docsSaveImmediately": <on>}`. The
 *  owner's; the daemon refuses anyone else. It takes effect after K2 restarts. */
export async function setSaveImmediately(on: boolean, scope: ServerScope = primaryScope()): Promise<void> {
  await daemonCliPost<unknown>(scope, 'settings/update', { docsSaveImmediately: on })
}

/** Board files in a tab on or off: `{"boardFilePreview": <on>}`. The owner's;
 *  the daemon refuses anyone else. */
export async function setBoardFilePreview(on: boolean, scope: ServerScope = primaryScope()): Promise<void> {
  await daemonCliPost<unknown>(scope, 'settings/update', { boardFilePreview: on })
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

/** A core in plain words, renderer-side, for the cores this client knows:
 *  the daemon's names ("Markdown core") and its Board text name internal
 *  parts, so Settings says what kind of file it is. A core this client
 *  doesn't know keeps the daemon's own words. `sentence` is the name inside
 *  a sentence ("text and code files"). */
export interface PlainCore {
  name: string
  description: string
  sentence: string
}

const PLAIN_CORES: Record<string, PlainCore> = {
  markdown: {
    name: 'Markdown files',
    description: '.md and .markdown files open beside the workspace’s agent, with live editing, presence and history.',
    sentence: 'Markdown files',
  },
  text: {
    name: 'Text and code files',
    description: 'Code, config, plain text, logs, HTML, XML, SVG and diagram sources, and CSV or TSV files shown as text.',
    sentence: 'text and code files',
  },
  'any-file': {
    name: 'Any other file',
    description:
      'PSD, CAD, 3D and .blend files, images, PDFs, office files and the rest. They open the usual way, with a panel beside them: who’s here, a lock, versions and restore, conflict copies and approvals.',
    sentence: 'any other file',
  },
  board: {
    name: 'Boards',
    description:
      'A whiteboard Garden for you and your agents, single-person for now. When this is off, boards are hidden and agents can’t open them. Your boards are kept.',
    sentence: 'boards',
  },
}

export function plainCore(core: Pick<DocCore, 'id' | 'name' | 'description'>): PlainCore {
  return PLAIN_CORES[core.id] ?? { name: core.name, description: core.description, sentence: core.name }
}

/** The turn-off dialog's title: "Turn off multiplayer for Markdown files?" */
export function turnOffTitle(core: Pick<DocCore, 'id' | 'name' | 'description'>): string {
  return `Turn off multiplayer for ${plainCore(core).sentence}?`
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
export function failedWriteBackLine(core: Pick<DocCore, 'id' | 'name' | 'description'>, f: DocCoreFailedPath): string {
  const kind = plainCore(core).sentence
  return (
    `${f.path} couldn't be written when multiplayer was turned off for ${kind} (${f.error}). ` +
    `Its last changes are kept in K2's history, and turning multiplayer on again for ${kind} writes them.`
  )
}

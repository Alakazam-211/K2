// prd-garden-access-v1 §5 G14, §6 G25–G29 (A5): the renderer's one typed
// client for Garden access. Every route shape the Settings page, the history
// modals and Zen's widget ⋯ menu read is parsed HERE, so a daemon shape
// change is a one-file fix.
//
// Thin client (CLAUDE.md, daemon-first): nothing here decides who may edit
// what. It reads the daemon's truth and posts the person's gestures. Every
// call goes to THIS computer's daemon (`zenLocalScope()`, owner token):
// Gardens live on this computer.
//
//   GET  /cli/settings/get                         `gardenAccess` (people only; absent = no such switch)
//   POST /cli/settings/update {gardenAccess}       the owner's switch
//   GET  /cli/zen/garden/access?garden=            the list (+ who made it)
//   POST /cli/zen/garden/access {garden, agent, allow}
//   GET  /cli/zen/garden/history?garden=&before=&limit=&kind=
//   GET  /cli/zen/widget/history?widget=&before=&limit=&kind=
//   POST /cli/zen/widget/sign {widget, garden}     the modals' Sign
//   GET  /cli/zen/gardens/deleted                  Deleted Gardens
//   POST /cli/zen/garden/restore {garden}
//   GET  /cli/zen/snapshot?file=&name=             View this version
//   POST /cli/zen/reset {garden|widget, to}        Restore a version (today's route)
//   GET  /cli/zen/widgets                          placed widgets + last signer
//   GET  /cli/projects/list                        this computer's agents (the Add picker)
//
// The shapes not yet pushed by A1/A2 are the ones in the lane's
// `a5-asks.md`; the switch is `gardenAccess` (default off).

import { create } from 'zustand'
import { listen } from '@tauri-apps/api/event'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { zenLocalScope } from './zen-api'

// ── Small parse helpers ─────────────────────────────────────────────

export class GardenAccessParseError extends Error {
  constructor(message: string) {
    super(`Garden access: ${message}`)
    this.name = 'GardenAccessParseError'
  }
}

function obj(v: unknown): Record<string, unknown> | null {
  return v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null
}

function str(o: Record<string, unknown>, k: string): string | null {
  const v = o[k]
  return typeof v === 'string' && v !== '' ? v : null
}

function num(o: Record<string, unknown>, k: string): number | null {
  const v = o[k]
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

function need<T>(v: T | null, what: string): T {
  if (v === null) throw new GardenAccessParseError(`${what} is missing`)
  return v
}

// ── The switch ──────────────────────────────────────────────────────

/** `gardenAccess` from a `settings/get` (or `settings/update`) body: true /
 *  false, or null when the daemon has no such switch (before 0.46.3 r9). */
export function gardenAccessSetting(body: unknown): boolean | null {
  const o = obj(body)
  if (!o || !('gardenAccess' in o)) return null
  if (typeof o.gardenAccess !== 'boolean') throw new GardenAccessParseError('"gardenAccess" is not true or false')
  return o.gardenAccess
}

export interface GardenAccessSwitchState {
  /** null = not read yet, or this daemon has no switch (today's page). */
  on: boolean | null
  loaded: boolean
}

export const useGardenAccessSwitchStore = create<GardenAccessSwitchState>(() => ({ on: null, loaded: false }))

let switchSeq = 0

/** Read the switch from this computer's daemon into the store. A failed
 *  read leaves today's page (`on: null`) and says why in the console; the
 *  page never guesses "on". */
export async function loadGardenAccessSwitch(): Promise<boolean | null> {
  const seq = ++switchSeq
  try {
    const on = gardenAccessSetting(await daemonCliGet<unknown>(zenLocalScope(), 'settings/get'))
    if (seq === switchSeq) useGardenAccessSwitchStore.setState({ on, loaded: true })
    return on
  } catch (err) {
    console.warn('[garden-access] reading the switch failed', err)
    if (seq === switchSeq) useGardenAccessSwitchStore.setState({ on: null, loaded: true })
    return null
  }
}

/** The owner's switch ("Garden access lists"). Returns what the daemon stored. */
export async function setGardenAccessSwitch(on: boolean): Promise<boolean> {
  const body = await daemonCliPost<unknown>(zenLocalScope(), 'settings/update', { gardenAccess: on })
  const stored = gardenAccessSetting(body)
  if (stored === null) throw new Error('settings/update: the daemon did not answer with "gardenAccess"')
  switchSeq++
  useGardenAccessSwitchStore.setState({ on: stored, loaded: true })
  return stored
}

/** True for a `sync:settings` payload about the switch. */
export function isGardenAccessSync(payload: unknown): boolean {
  const o = obj(payload)
  return !!o && 'gardenAccess' in o
}

/** Keep the switch live while a page that draws it is mounted: read now,
 *  on a `sync:settings {gardenAccess}` flip and on window focus. No polling.
 *  Returns the cleanup. */
export function watchGardenAccessSwitch(): () => void {
  void loadGardenAccessSwitch()
  let off: (() => void) | null = null
  let cancelled = false
  void listen<unknown>('sync:settings', (e) => {
    if (isGardenAccessSync(e.payload)) void loadGardenAccessSwitch()
  })
    .then((u) => {
      if (cancelled) u()
      else off = u
    })
    .catch((err) => console.warn('[garden-access] sync:settings listen failed', err))
  const onFocus = (): void => void loadGardenAccessSwitch()
  window.addEventListener('focus', onFocus)
  return () => {
    cancelled = true
    off?.()
    window.removeEventListener('focus', onFocus)
  }
}

/** Tests only. */
export function __resetGardenAccessForTests(): void {
  switchSeq = 0
  useGardenAccessSwitchStore.setState({ on: null, loaded: false })
}

// ── Actors ──────────────────────────────────────────────────────────

export type GardenActorKind = 'owner' | 'login' | 'agent' | 'shell' | 'k2' | 'disk' | 'migration'

export interface GardenActor {
  kind: GardenActorKind
  /** The project id (agent, shell) or the login. */
  id: string | null
  handle: string | null
  name: string | null
  color: string | null
}

const ACTOR_KINDS: readonly GardenActorKind[] = ['owner', 'login', 'agent', 'shell', 'k2', 'disk', 'migration']

export function parseGardenActor(raw: unknown, at: string): GardenActor {
  const o = obj(raw)
  if (!o) throw new GardenAccessParseError(`${at} is not an actor`)
  const kind = str(o, 'kind')
  if (!kind || !(ACTOR_KINDS as readonly string[]).includes(kind)) {
    throw new GardenAccessParseError(`${at}.kind "${String(o.kind)}" is not an actor kind`)
  }
  return {
    kind: kind as GardenActorKind,
    id: str(o, 'id'),
    handle: str(o, 'handle'),
    name: str(o, 'name'),
    color: str(o, 'color'),
  }
}

/** Who did it, in the modal's words (G26a): "you", the agent's name, "a
 *  terminal in X's room", "K2", "outside K2". */
export function gardenActorWords(a: GardenActor): string {
  const who = a.name ?? a.handle ?? a.id ?? 'an agent'
  switch (a.kind) {
    case 'owner':
      return 'You'
    case 'login':
      return a.name ?? a.id ?? 'A login'
    case 'agent':
      return who
    case 'shell':
      return `A terminal in ${who}’s room`
    case 'k2':
    case 'migration':
      return 'K2'
    case 'disk':
      return 'Outside K2'
  }
}

// ── Dates ───────────────────────────────────────────────────────────

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** "12 Oct" (this year) or "12 Oct 2025", in local time. `at` is unix seconds. */
export function gardenDay(at: number, now: Date = new Date()): string {
  const d = new Date(at * 1000)
  const day = `${d.getDate()} ${MONTHS[d.getMonth()]}`
  return d.getFullYear() === now.getFullYear() ? day : `${day} ${d.getFullYear()}`
}

/** "12 Oct, 14:05": a history line's time. */
export function gardenWhen(at: number, now: Date = new Date()): string {
  const d = new Date(at * 1000)
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  return `${gardenDay(at, now)}, ${hh}:${mm}`
}

// ── The list (G14) ──────────────────────────────────────────────────

export type GardenAddedBy = 'owner' | 'card' | 'migration'

export interface GardenEditor {
  /** The workspace (project) id. */
  agent: string
  handle: string
  name: string
  color: string | null
  addedBy: GardenAddedBy
  addedAt: number
}

export interface GardenMaker {
  agent: string
  handle: string
  name: string
  at: number | null
}

export interface GardenAccessList {
  garden: string
  name: string
  made: GardenMaker | null
  editors: GardenEditor[]
}

function parseMaker(raw: unknown, at: string): GardenMaker | null {
  if (raw === null || raw === undefined) return null
  const o = obj(raw)
  if (!o) throw new GardenAccessParseError(`${at} is not an object`)
  const agent = need(str(o, 'agent'), `${at}.agent`)
  const handle = str(o, 'handle') ?? agent
  return { agent, handle, name: str(o, 'name') ?? handle, at: num(o, 'at') }
}

export function parseGardenAccessList(raw: unknown): GardenAccessList {
  const o = obj(raw)
  if (!o) throw new GardenAccessParseError('the access answer is not an object')
  if (!Array.isArray(o.editors)) throw new GardenAccessParseError('no "editors" in the access answer')
  const garden = need(str(o, 'garden'), 'garden')
  const editors = o.editors.map((e, i): GardenEditor => {
    const r = obj(e)
    if (!r) throw new GardenAccessParseError(`editors[${i}] is not an object`)
    const agent = need(str(r, 'agent'), `editors[${i}].agent`)
    const addedBy = str(r, 'addedBy')
    if (addedBy !== 'owner' && addedBy !== 'card' && addedBy !== 'migration') {
      throw new GardenAccessParseError(`editors[${i}].addedBy "${String(r.addedBy)}" is not owner, card or migration`)
    }
    const handle = str(r, 'handle') ?? agent
    return {
      agent,
      handle,
      name: str(r, 'name') ?? handle,
      color: str(r, 'color'),
      addedBy,
      addedAt: need(num(r, 'addedAt'), `editors[${i}].addedAt`),
    }
  })
  return { garden, name: str(o, 'name') ?? garden, made: parseMaker(o.made, 'made'), editors }
}

/** "added by you · 12 Oct", "added when you allowed its card · 12 Oct",
 *  "added automatically at upgrade · 22 Oct" (G26, G4). */
export function gardenEditorAddedWords(e: Pick<GardenEditor, 'addedBy' | 'addedAt'>, now?: Date): string {
  const how =
    e.addedBy === 'owner' ? 'added by you' : e.addedBy === 'card' ? 'added when you allowed its card' : 'added automatically at upgrade'
  return `${how} · ${gardenDay(e.addedAt, now)}`
}

export async function fetchGardenAccess(garden: string): Promise<GardenAccessList> {
  return parseGardenAccessList(await daemonCliGet<unknown>(zenLocalScope(), 'zen/garden/access', { garden }))
}

/** Add (`allow: true`) or remove an agent. Returns whether it is listed now. */
export async function setGardenEditor(garden: string, agent: string, allow: boolean): Promise<boolean> {
  const body = obj(await daemonCliPost<unknown>(zenLocalScope(), 'zen/garden/access', { garden, agent, allow }))
  if (!body || typeof body.listed !== 'boolean') {
    throw new Error('zen/garden/access: the daemon did not answer with "listed"')
  }
  return body.listed
}

// ── Sidebar facts on `GET zen/gardens` (G25) ────────────────────────

export interface GardenListFacts {
  /** null = the daemon sent no count (switch off, or older). */
  editors: number | null
  madeBy: { agent: string; handle: string; name: string } | null
}

/** The extra facts a `zen/gardens` row carries with the switch on. */
export function gardenListFacts(raw: unknown): Map<string, GardenListFacts> {
  const out = new Map<string, GardenListFacts>()
  const o = obj(raw)
  if (!o || !Array.isArray(o.gardens)) return out
  for (const g of o.gardens) {
    const r = obj(g)
    const id = r && str(r, 'id')
    if (!r || !id) continue
    const m = obj(r.madeBy)
    const agent = m && str(m, 'agent')
    out.set(id, {
      editors: num(r, 'editors'),
      madeBy:
        m && agent
          ? { agent, handle: str(m, 'handle') ?? agent, name: str(m, 'name') ?? str(m, 'handle') ?? agent }
          : null,
    })
  }
  return out
}

export async function fetchGardenListFacts(): Promise<Map<string, GardenListFacts>> {
  return gardenListFacts(await daemonCliGet<unknown>(zenLocalScope(), 'zen/gardens'))
}

// ── History (G26a, G29) ─────────────────────────────────────────────

export type GardenHistorySource = 'event' | 'snapshot' | 'stamp' | 'proposal'

export interface GardenHistoryItem {
  id: string
  at: number
  kind: string
  source: GardenHistorySource
  actor: GardenActor
  widget: string | null
  detail: Record<string, unknown>
  snapshot: { file: string; name: string } | null
  /** K2's own words for the line, when the daemon sends them. */
  summary: string | null
}

export interface GardenUnsigned {
  widget: string
  hash: string
  /** The Gardens it fails the signer gate in. */
  gardens: string[]
}

export interface GardenHistoryPage {
  items: GardenHistoryItem[]
  next: string | null
  unsigned: GardenUnsigned[]
}

export type GardenHistoryFilter = 'all' | 'changes' | 'access' | 'asks'

export const GARDEN_HISTORY_FILTERS: ReadonlyArray<{ id: GardenHistoryFilter; label: string }> = [
  { id: 'all', label: 'All' },
  { id: 'changes', label: 'Changes' },
  { id: 'access', label: 'Access' },
  { id: 'asks', label: 'Asks' },
]

/** The G1 event kinds behind each filter (null = every kind). */
export const GARDEN_HISTORY_FILTER_KINDS: Record<GardenHistoryFilter, readonly string[] | null> = {
  all: null,
  changes: [
    'made',
    'changed',
    'widget_changed',
    'renamed',
    'reordered',
    'theme',
    'reset',
    'template',
    'locked',
    'unlocked',
    'sync',
    'deleted',
    'restored',
    'disk_edit',
    'lock_put_back',
    'signed',
    'imported',
  ],
  access: ['access_added', 'access_removed'],
  asks: ['access_asked', 'access_not_now', 'delete_asked', 'delete_kept'],
}

const SOURCES: readonly GardenHistorySource[] = ['event', 'snapshot', 'stamp', 'proposal']

export function parseGardenHistory(raw: unknown): GardenHistoryPage {
  const o = obj(raw)
  if (!o) throw new GardenAccessParseError('the history answer is not an object')
  if (!Array.isArray(o.items)) throw new GardenAccessParseError('no "items" in the history answer')
  const items = o.items.map((it, i): GardenHistoryItem => {
    const r = obj(it)
    if (!r) throw new GardenAccessParseError(`items[${i}] is not an object`)
    const source = str(r, 'source') ?? 'event'
    if (!(SOURCES as readonly string[]).includes(source)) {
      throw new GardenAccessParseError(`items[${i}].source "${source}" is not known`)
    }
    const snap = obj(r.snapshot)
    const file = snap && str(snap, 'file')
    const name = snap && str(snap, 'name')
    return {
      id: need(str(r, 'id') ?? (typeof r.id === 'number' ? String(r.id) : null), `items[${i}].id`),
      at: need(num(r, 'at'), `items[${i}].at`),
      kind: need(str(r, 'kind'), `items[${i}].kind`),
      source: source as GardenHistorySource,
      actor: parseGardenActor(r.actor, `items[${i}].actor`),
      widget: str(r, 'widget'),
      detail: obj(r.detail) ?? {},
      snapshot: file && name ? { file, name } : null,
      summary: str(r, 'summary'),
    }
  })
  const unsigned: GardenUnsigned[] = []
  if (Array.isArray(o.unsigned)) {
    o.unsigned.forEach((u, i) => {
      const r = obj(u)
      if (!r) throw new GardenAccessParseError(`unsigned[${i}] is not an object`)
      unsigned.push({
        widget: need(str(r, 'widget'), `unsigned[${i}].widget`),
        hash: str(r, 'hash') ?? '',
        gardens: Array.isArray(r.gardens) ? r.gardens.filter((g): g is string => typeof g === 'string') : [],
      })
    })
  }
  return { items, next: typeof o.next === 'string' && o.next ? o.next : null, unsigned }
}

function d(item: GardenHistoryItem, k: string): string | null {
  return str(item.detail, k)
}

/** What happened, in the modal's words (after the actor): "changed the
 *  layout", "added ana", "asked to edit". A daemon `summary` wins. */
export function gardenEventWords(item: GardenHistoryItem): string {
  if (item.summary) return item.summary
  const w = item.widget ?? d(item, 'widget')
  const agent = d(item, 'name') ?? d(item, 'handle') ?? d(item, 'agent')
  if (item.source === 'snapshot') return w ? `saved a version of ${w}` : 'saved a version'
  if (item.source === 'proposal') {
    const decision = d(item, 'decision') ?? d(item, 'state')
    return decision === 'rejected' ? 'had a proposal rejected' : 'had a proposal approved'
  }
  switch (item.kind) {
    case 'made':
      return 'made this Garden'
    case 'changed':
      return 'changed the layout'
    case 'widget_changed':
      return w ? `changed ${w}` : 'changed a widget'
    case 'renamed': {
      const from = d(item, 'old') ?? d(item, 'from')
      const to = d(item, 'new') ?? d(item, 'to')
      return from && to ? `renamed it from “${from}” to “${to}”` : 'renamed it'
    }
    case 'reordered':
      return 'moved it in the Garden list'
    case 'theme':
      return d(item, 'theme') ? `changed the theme to ${d(item, 'theme')}` : 'changed the theme'
    case 'reset':
      return w ? `reset ${w}` : 'reset it'
    case 'template':
      return 'started it from a template'
    case 'locked':
      return 'locked it'
    case 'unlocked':
      return 'unlocked it'
    case 'sync':
      return 'changed design updates'
    case 'access_added':
      return item.actor.kind === 'migration'
        ? `added ${agent ?? 'an agent'} automatically at upgrade`
        : `added ${agent ?? 'an agent'}`
    case 'access_removed':
      return `removed ${agent ?? 'an agent'}`
    case 'access_asked':
      return 'asked to edit'
    case 'access_not_now':
      return `said Not now to ${agent ?? 'an agent'}`
    case 'delete_asked':
      return 'asked to delete it'
    case 'delete_kept':
      return `kept it (${agent ?? 'an agent'} asked to delete it)`
    case 'deleted':
      return 'deleted it'
    case 'restored':
      return 'restored it'
    case 'disk_edit':
      return w ? `changed ${w}` : 'changed it'
    case 'lock_put_back':
      return 'had a change put back (the Garden is locked)'
    case 'signed':
      return w ? `signed ${w}` : 'signed this version'
    case 'imported':
      return d(item, 'line') ?? 'an older Access Audit line'
    default:
      return item.kind.replace(/_/g, ' ')
  }
}

/** The "added automatically at upgrade" line reads as its own sentence. */
export function gardenHistoryLine(item: GardenHistoryItem): string {
  if (item.kind === 'access_added' && item.actor.kind === 'migration' && !item.summary) {
    const agent = d(item, 'name') ?? d(item, 'handle') ?? d(item, 'agent') ?? 'An agent'
    return `${agent} was added automatically at upgrade`
  }
  return `${gardenActorWords(item.actor)} ${gardenEventWords(item)}`
}

export interface GardenHistoryQuery {
  before?: string | null
  filter?: GardenHistoryFilter
  limit?: number
}

function historyParams(q: GardenHistoryQuery): Record<string, string | number> {
  const p: Record<string, string | number> = { limit: q.limit ?? 50 }
  if (q.before) p.before = q.before
  const kinds = GARDEN_HISTORY_FILTER_KINDS[q.filter ?? 'all']
  if (kinds) p.kind = kinds.join(',')
  return p
}

/** A Garden's history (its events plus its placed widgets'); `'*'` is the
 *  computer-wide look (Styles). */
export async function fetchGardenHistory(garden: string, q: GardenHistoryQuery = {}): Promise<GardenHistoryPage> {
  return parseGardenHistory(
    await daemonCliGet<unknown>(zenLocalScope(), 'zen/garden/history', { garden, ...historyParams(q) }),
  )
}

export async function fetchWidgetHistory(widget: string, q: GardenHistoryQuery = {}): Promise<GardenHistoryPage> {
  return parseGardenHistory(
    await daemonCliGet<unknown>(zenLocalScope(), 'zen/widget/history', { widget, ...historyParams(q) }),
  )
}

/** The modals' Sign (G14, G26a): the person signs the live version for `garden`. */
export async function signGardenWidget(widget: string, garden: string): Promise<void> {
  await daemonCliPost<unknown>(zenLocalScope(), 'zen/widget/sign', { widget, garden })
}

/** View this version: the snapshot's text, read-only. */
export async function fetchSnapshotText(snapshot: { file: string; name: string }): Promise<string> {
  const o = obj(await daemonCliGet<unknown>(zenLocalScope(), 'zen/snapshot', { file: snapshot.file, name: snapshot.name }))
  if (!o) throw new GardenAccessParseError('the snapshot answer is not an object')
  if (typeof o.text === 'string') return o.text
  if (Array.isArray(o.files)) {
    return o.files
      .map((f) => {
        const r = obj(f)
        return r ? `── ${String(r.path ?? '')}\n${String(r.text ?? '')}` : ''
      })
      .join('\n\n')
  }
  throw new GardenAccessParseError('no "text" in the snapshot answer')
}

/** Restore a version: today's `zen/reset {garden|widget, to}` (the current
 *  one is kept as a snapshot first). */
export async function restoreGardenVersion(
  target: { garden: string } | { widget: string },
  snapshotName: string,
): Promise<void> {
  await daemonCliPost<unknown>(zenLocalScope(), 'zen/reset', { ...target, to: snapshotName })
}

// ── Deleted Gardens (G25) ───────────────────────────────────────────

export interface DeletedGarden {
  id: string
  name: string
  deletedAt: number | null
  deletedBy: GardenActor | null
}

export function parseDeletedGardens(raw: unknown): DeletedGarden[] {
  const o = obj(raw)
  if (!o || !Array.isArray(o.gardens)) throw new GardenAccessParseError('no "gardens" in the deleted answer')
  return o.gardens.map((g, i) => {
    const r = obj(g)
    if (!r) throw new GardenAccessParseError(`gardens[${i}] is not an object`)
    const id = need(str(r, 'id'), `gardens[${i}].id`)
    return {
      id,
      name: str(r, 'name') ?? id,
      deletedAt: num(r, 'deletedAt'),
      deletedBy: r.deletedBy ? parseGardenActor(r.deletedBy, `gardens[${i}].deletedBy`) : null,
    }
  })
}

/** Deleted Gardens, or null when this daemon has no such route (the block
 *  hides; PRD §9 lists it as cuttable). Any other failure throws. */
export async function fetchDeletedGardens(): Promise<DeletedGarden[] | null> {
  try {
    return parseDeletedGardens(await daemonCliGet<unknown>(zenLocalScope(), 'zen/gardens/deleted'))
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    if (/unknown zen route/i.test(msg)) return null
    throw err
  }
}

export async function restoreDeletedGarden(garden: string): Promise<void> {
  await daemonCliPost<unknown>(zenLocalScope(), 'zen/garden/restore', { garden })
}

// ── Placed widgets and their last signer (G26) ──────────────────────

export interface GardenWidgetSigner {
  kind: 'owner' | 'k2' | 'agent'
  handle: string | null
  name: string | null
  at: number | null
}

export interface GardenWidgetLine {
  widget: string
  title: string | null
  builtin: boolean
  signedBy: GardenWidgetSigner | null
  changedAt: number | null
  /** Per placing Garden: does the live version pass the signer gate there?
   *  Absent from a daemon with the switch off. */
  signedIn: Record<string, boolean | undefined>
  gardens: string[]
}

export function parseGardenWidgets(raw: unknown): GardenWidgetLine[] {
  const o = obj(raw)
  if (!o || !Array.isArray(o.widgets)) throw new GardenAccessParseError('no "widgets" in the widget list')
  const out: GardenWidgetLine[] = []
  for (const w of o.widgets) {
    const r = obj(w)
    const name = r && str(r, 'name')
    if (!r || !name) continue
    const s = obj(r.signedBy)
    const kind = s && str(s, 'kind')
    const signedIn: Record<string, boolean | undefined> = {}
    const gardens: string[] = []
    if (Array.isArray(r.placements)) {
      for (const p of r.placements) {
        const pr = obj(p)
        const g = pr && str(pr, 'garden')
        if (!pr || !g) continue
        if (!gardens.includes(g)) gardens.push(g)
        if (typeof pr.signed === 'boolean') signedIn[g] = signedIn[g] === false ? false : pr.signed
      }
    }
    out.push({
      widget: name,
      title: str(r, 'title'),
      builtin: r.builtin === true,
      signedBy:
        s && (kind === 'owner' || kind === 'k2' || kind === 'agent')
          ? { kind, handle: str(s, 'handle'), name: str(s, 'name'), at: num(s, 'at') }
          : null,
      changedAt: num(r, 'changedAt'),
      signedIn,
      gardens,
    })
  }
  return out
}

export async function fetchGardenWidgets(): Promise<GardenWidgetLine[]> {
  return parseGardenWidgets(await daemonCliGet<unknown>(zenLocalScope(), 'zen/widgets'))
}

/** "last signed by ana · 21 Oct" / "last signed by you · 21 Oct" /
 *  "last changed 22 Oct" (G26). Never "not signed": that lives in the modal. */
export function gardenWidgetSignerWords(w: Pick<GardenWidgetLine, 'signedBy' | 'changedAt'>, now?: Date): string {
  const s = w.signedBy
  if (s) {
    const who = s.kind === 'owner' ? 'you' : s.kind === 'k2' ? 'K2' : (s.name ?? s.handle ?? 'an agent')
    return s.at !== null ? `last signed by ${who} · ${gardenDay(s.at, now)}` : `last signed by ${who}`
  }
  return w.changedAt !== null ? `last changed ${gardenDay(w.changedAt, now)}` : ''
}

// ── This computer's agents (the Add picker) ─────────────────────────

export interface LocalAgent {
  id: string
  name: string
  path: string
  color: string
  iconUrl: string | null
  focusGroupId: string | null
}

export function parseLocalAgents(raw: unknown): LocalAgent[] {
  if (!Array.isArray(raw)) throw new GardenAccessParseError('projects/list did not return a list')
  const out: LocalAgent[] = []
  for (const p of raw) {
    const r = obj(p)
    const id = r && str(r, 'id')
    if (!r || !id) continue
    out.push({
      id,
      name: str(r, 'name') ?? id,
      path: str(r, 'path') ?? '',
      color: str(r, 'color') ?? '#888888',
      iconUrl: str(r, 'iconUrl'),
      focusGroupId: str(r, 'focusGroupId'),
    })
  }
  return out
}

/** This computer's workspaces (agents), read when the picker opens (a
 *  gesture, never a render path, and never the window's projects store:
 *  the window may be on another server). */
export async function fetchLocalAgents(): Promise<LocalAgent[]> {
  return parseLocalAgents(await daemonCliGet<unknown>(zenLocalScope(), 'projects/list'))
}

// ── Refusals, in words ──────────────────────────────────────────────

export function gardenAccessErrorText(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err)
  if (/owner_only|gardens_owner|person_only/.test(msg)) return 'Only the computer’s owner can change who edits a Garden.'
  if (/unknown_agent/.test(msg)) return 'That agent is gone. It may have been deleted.'
  if (/unknown_garden/.test(msg)) return 'That Garden is gone. It may have been deleted somewhere else.'
  if (/not_available/.test(msg)) return 'Garden access lists are off on this computer.'
  if (/unknown zen route/i.test(msg)) return 'K2 on this computer is older than this app. Update it to use Garden access.'
  return msg || 'Something went wrong.'
}

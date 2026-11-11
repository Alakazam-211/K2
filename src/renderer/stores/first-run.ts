// Setup checklist (prd-setup-checklist-v1, contract `setup-checklist-contract.md`),
// per server. The daemon works out every row from real state, for the
// caller, and keeps this person's hidden / skipped / finished marks. This
// store renders that answer and sends the three gestures (hide, show,
// skip); it never decides a row itself.
//
// It re-reads `GET /cli/first-run/status` on `first_run_changed` and on the
// events that change a row's state (LLM tokens, workspaces, the tunnel, a
// paired phone), and never polls. Same shape as `llm-accounts.ts`: entries
// keyed by `scope.id`, an epoch per key so a load that lands after a server
// switch lands nowhere, and the window's entry dropped on a switch.

import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { onActiveHostChange } from '@/stores/connect-host'
import {
  onAppHello,
  onFirstRunChanged,
  onLlmAccountsChanged,
  onPairCodeChanged,
  onProjectsChanged,
  onTunnelIdentityChanged,
  onTunnelStatusChanged,
} from '@/stores/session-events'

export const FIRST_RUN_STATUS_PATH = 'first-run/status'
export const FIRST_RUN_HIDE_PATH = 'first-run/hide'
export const FIRST_RUN_SHOW_PATH = 'first-run/show'
export const FIRST_RUN_SKIP_PATH = 'first-run/skip'
/** K5 at first run (prd-setup-checklist-v1 §10, `k5-first-run-contract.md`). */
export const FIRST_RUN_OFFER_DISMISS_PATH = 'first-run/offer/dismiss'
export const FIRST_RUN_K5_PATH = 'first-run/k5'
export const FIRST_RUN_COS_START_PATH = 'first-run/chief-of-staff/start'

/** Row ids this client knows, in the contract's order. `chief_of_staff`
 *  takes `first_agent`'s place while the server's K5 switch is on; the
 *  daemon sends one or the other. Unknown ids are dropped. */
export const FIRST_RUN_ROW_IDS = ['llm_sign_in', 'chief_of_staff', 'first_agent', 'tunnel', 'phone', 'own_sign_in'] as const
export type FirstRunRowId = (typeof FIRST_RUN_ROW_IDS)[number]

/** What a row's button opens. Unknown values are dropped (the row stays,
 *  without an opener). */
export const FIRST_RUN_OPENS = [
  'llm_sign_in',
  'add_workspace',
  'tunnel',
  'phone',
  'owner_account_prompt',
  'k5_sheet',
  'k5_thread_cos',
] as const
export type FirstRunOpens = (typeof FIRST_RUN_OPENS)[number]

export type FirstRunRowState = 'done' | 'todo' | 'skipped'
export const FIRST_RUN_NOTES = ['needs_tunnel', 'needs_k5'] as const
export type FirstRunNote = (typeof FIRST_RUN_NOTES)[number]

/** The offer the LLM sign-in sheet shows once a tool is signed in. */
export type FirstRunOffer = 'meet_k5'

/** The server's live K5 (the `orchestrator` seat). */
export interface K5Seat {
  workspaceId: string
  name: string
  path: string
}

/** What the K5 sheet starts from: name, handle and folder, all editable. */
export interface K5Defaults {
  name: string
  handle: string
  path: string
}

export interface FirstRunRow {
  id: FirstRunRowId
  state: FirstRunRowState
  opens: FirstRunOpens | null
  note: FirstRunNote | null
}

export interface FirstRunDoc {
  /** The server's switch (off by default). Off: the app shows nothing. */
  enabled: boolean
  hidden: boolean
  finished: boolean
  rows: FirstRunRow[]
  /** `meet_k5` while the daemon says to offer K5 to this person, else null. */
  offer: FirstRunOffer | null
  /** Null when there's no live K5. */
  k5: K5Seat | null
  /** Present while the server's K5 switch is on. */
  k5Defaults: K5Defaults | null
}

export interface FirstRunEntry {
  doc: FirstRunDoc | null
  error: string | null
}

function isOneOf<T extends string>(list: readonly T[], v: unknown): v is T {
  return typeof v === 'string' && (list as readonly string[]).includes(v)
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.trim() !== '' ? v : null
}

function parseK5Seat(raw: unknown): K5Seat | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const o = raw as Record<string, unknown>
  const workspaceId = str(o.workspaceId)
  if (!workspaceId) return null
  return { workspaceId, name: str(o.name) ?? 'K5', path: str(o.path) ?? '' }
}

function parseK5Defaults(raw: unknown): K5Defaults | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const o = raw as Record<string, unknown>
  const name = str(o.name)
  const handle = str(o.handle)
  if (!name || !handle) return null
  return { name, handle, path: str(o.path) ?? '' }
}

/** Parse the route's body defensively: unknown row ids, `opens` values,
 *  notes and offers are dropped, an unknown state reads as `todo`, and a
 *  malformed body is a switched-off checklist (never a crash). */
export function parseFirstRunDoc(raw: unknown): FirstRunDoc {
  const o = raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
  const rows: FirstRunRow[] = []
  const seen = new Set<string>()
  for (const r of Array.isArray(o.rows) ? o.rows : []) {
    if (!r || typeof r !== 'object') continue
    const row = r as Record<string, unknown>
    if (!isOneOf(FIRST_RUN_ROW_IDS, row.id) || seen.has(row.id)) continue
    seen.add(row.id)
    rows.push({
      id: row.id,
      state: row.state === 'done' || row.state === 'skipped' ? row.state : 'todo',
      opens: isOneOf(FIRST_RUN_OPENS, row.opens) ? row.opens : null,
      note: isOneOf(FIRST_RUN_NOTES, row.note) ? row.note : null,
    })
  }
  return {
    enabled: o.enabled === true,
    hidden: o.hidden === true,
    finished: o.finished === true,
    rows,
    offer: o.offer === 'meet_k5' ? 'meet_k5' : null,
    k5: parseK5Seat(o.k5),
    k5Defaults: parseK5Defaults(o.k5Defaults),
  }
}

/** The rows to draw, or null when the checklist shows nothing: the switch
 *  is off, the person hid it, it's finished, or no row is left to show. */
export function visibleRows(doc: FirstRunDoc | null | undefined): FirstRunRow[] | null {
  if (!doc || !doc.enabled || doc.hidden || doc.finished || doc.rows.length === 0) return null
  return doc.rows
}

/** Whether the hidden list sits as its "Get started" bubble: the switch is
 *  on, the person hid it, it isn't finished, and a row is left to show. The
 *  bubble isn't the list, so `isSetupChecklistShowing` stays false for it
 *  (the sidebar keeps its normal range). */
export function showsCollapsed(doc: FirstRunDoc | null | undefined): boolean {
  return !!doc && doc.enabled && doc.hidden && !doc.finished && doc.rows.length > 0
}

function checklistShowingIn(entries: Record<string, FirstRunEntry>): boolean {
  return visibleRows(entries[primaryScope().id]?.doc) !== null
}

/** Whether the setup checklist is drawn for the window's server right now
 *  (its rows, not the "You're set up" moment). For stores and handlers. */
export function isSetupChecklistShowing(): boolean {
  return checklistShowingIn(useFirstRunStore.getState().entries)
}

/** `isSetupChecklistShowing`, as a hook. It reads the store only; the
 *  checklist and its rail button are what watch the server. */
export function useSetupChecklistShowing(): boolean {
  return useFirstRunStore((s) => checklistShowingIn(s.entries))
}

/** Rows still to do (not done, not skipped). */
export function rowsLeft(rows: readonly FirstRunRow[]): number {
  return rows.filter((r) => r.state === 'todo').length
}

function errorText(e: unknown): string {
  return (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
}

const loadInflight = new Map<string, Promise<void>>()
/** A load was asked for while one ran: run once more after it, so an event
 *  that arrived mid-request is never answered by the older read. */
const loadAgain = new Set<string>()
const epochs = new Map<string, number>()
let primaryWatchers = 0

function hostSwitched(e: unknown): boolean {
  return e instanceof Error && e.name === 'HostSwitchedError'
}

function epochOf(key: string): number {
  return epochs.get(key) ?? 0
}

function dropEntry(key: string): void {
  epochs.set(key, epochOf(key) + 1)
  loadInflight.delete(key)
  loadAgain.delete(key)
  useFirstRunStore.setState((s) => {
    if (!(key in s.entries)) return s
    const entries = { ...s.entries }
    delete entries[key]
    return { entries }
  })
}

interface FirstRunStore {
  entries: Record<string, FirstRunEntry>
  load: (scope?: ServerScope) => Promise<void>
  /** Hide the checklist for this person (`POST first-run/hide`). */
  hide: (scope?: ServerScope) => Promise<void>
  /** Bring it back: clears hidden and finished (`POST first-run/show`). */
  show: (scope?: ServerScope) => Promise<void>
  /** Skip one row for this person (`POST first-run/skip`). */
  skip: (row: FirstRunRowId, scope?: ServerScope) => Promise<void>
  /** "Not now" on the K5 offer, kept per person
   *  (`POST first-run/offer/dismiss`). */
  dismissOffer: (scope?: ServerScope) => Promise<void>
  /** Make K5 (`POST first-run/k5`). Resolves to the new seat; throws the
   *  daemon's refusal (the sheet shows it). The status is read again. */
  createK5: (req: CreateK5Request, scope?: ServerScope) => Promise<K5Seat>
  /** Post "Let's hire my Chief of Staff." into K5's Thread as this person
   *  and wake K5 (`POST first-run/chief-of-staff/start`). Resolves to K5's
   *  workspace id; throws the daemon's refusal. */
  startChiefOfStaff: (scope?: ServerScope) => Promise<string>
}

export interface CreateK5Request {
  /** `claude`, `codex` or `grok`: a tool signed in on this server. */
  tool: string
  name?: string
  handle?: string
}

/** POST a gesture and take the body it returns (the same shape as GET). A
 *  failure keeps the last list and says what failed. */
async function mutate(scope: ServerScope, route: string, body: Record<string, unknown>): Promise<void> {
  const key = scope.id
  const epoch = epochOf(key)
  try {
    const doc = parseFirstRunDoc(await daemonCliPost<unknown>(scope, route, body))
    if (epoch !== epochOf(key)) return
    useFirstRunStore.setState((s) => ({ entries: { ...s.entries, [key]: { doc, error: null } } }))
  } catch (e) {
    if (epoch !== epochOf(key) || hostSwitched(e)) return
    useFirstRunStore.setState((s) => ({
      entries: { ...s.entries, [key]: { doc: s.entries[key]?.doc ?? null, error: errorText(e) } },
    }))
  }
}

export const useFirstRunStore = create<FirstRunStore>((set, get) => ({
  entries: {},
  load: (scope = primaryScope()) => {
    const key = scope.id
    const running = loadInflight.get(key)
    if (running) {
      loadAgain.add(key)
      return running
    }
    const epoch = epochOf(key)
    const p = (async () => {
      try {
        const doc = parseFirstRunDoc(await daemonCliGet<unknown>(scope, FIRST_RUN_STATUS_PATH))
        if (epoch !== epochOf(key)) return
        set((s) => ({ entries: { ...s.entries, [key]: { doc, error: null } } }))
      } catch (e) {
        if (epoch !== epochOf(key) || hostSwitched(e)) return
        // An older daemon without the route, or a login it refuses: keep
        // the last answer from THIS server (none after a switch), so the
        // checklist stays off rather than guessing.
        set((s) => ({
          entries: { ...s.entries, [key]: { doc: s.entries[key]?.doc ?? null, error: errorText(e) } },
        }))
      } finally {
        if (epoch === epochOf(key)) {
          loadInflight.delete(key)
          if (loadAgain.delete(key)) void get().load(scope)
        }
      }
    })()
    loadInflight.set(key, p)
    return p
  },
  hide: (scope = primaryScope()) => mutate(scope, FIRST_RUN_HIDE_PATH, {}),
  show: (scope = primaryScope()) => mutate(scope, FIRST_RUN_SHOW_PATH, {}),
  skip: (row, scope = primaryScope()) => mutate(scope, FIRST_RUN_SKIP_PATH, { row }),
  dismissOffer: (scope = primaryScope()) => mutate(scope, FIRST_RUN_OFFER_DISMISS_PATH, {}),
  createK5: async (req, scope = primaryScope()) => {
    const body: Record<string, unknown> = { tool: req.tool }
    const name = req.name?.trim()
    const handle = req.handle?.trim()
    if (name) body.name = name
    if (handle) body.handle = handle
    const out = await daemonCliPost<unknown>(scope, FIRST_RUN_K5_PATH, body)
    void get().load(scope)
    const seat = parseK5Seat(out)
    if (!seat) throw new Error("The server didn't say where K5 is.")
    return seat
  },
  startChiefOfStaff: async (scope = primaryScope()) => {
    const out = await daemonCliPost<unknown>(scope, FIRST_RUN_COS_START_PATH, {})
    void get().load(scope)
    const id = out && typeof out === 'object' ? str((out as Record<string, unknown>).workspaceId) : null
    if (!id) throw new Error("The server didn't say where K5 is.")
    return id
  },
}))

/** The window switched servers: drop its checklist, then load the new
 *  server's if something is watching. */
export function resetWindowFirstRun(): void {
  const scope = primaryScope()
  dropEntry(scope.id)
  if (primaryWatchers > 0) void useFirstRunStore.getState().load(scope)
}

onActiveHostChange(() => resetWindowFirstRun())

export function resetFirstRunForTests(): void {
  for (const key of new Set([...epochs.keys(), ...loadInflight.keys()])) epochs.set(key, epochOf(key) + 1)
  loadInflight.clear()
  loadAgain.clear()
  primaryWatchers = 0
  useFirstRunStore.setState({ entries: {} })
}

/** Load now, and again on every event from that server that can change a
 *  row or the list: `first_run_changed`, `llm_accounts_changed`,
 *  `projects_changed`, `tunnel_status_changed`, `tunnel_identity_changed`,
 *  `pair_code_changed`, and each socket (re)connect (events missed while
 *  it was down). Returns the unsubscribe fn. */
export function watchFirstRun(scope: ServerScope = primaryScope()): () => void {
  const primary = scope.isPrimary
  if (primary) primaryWatchers += 1
  const reload = (): void => void useFirstRunStore.getState().load(scope)
  reload()
  const offs = [
    onFirstRunChanged(scope, reload),
    onLlmAccountsChanged(scope, reload),
    onProjectsChanged(scope, reload),
    onTunnelStatusChanged(scope, reload),
    onTunnelIdentityChanged(scope, reload),
    onPairCodeChanged(scope, reload),
    onAppHello(scope, reload),
  ]
  let done = false
  return () => {
    if (done) return
    done = true
    if (primary) primaryWatchers = Math.max(0, primaryWatchers - 1)
    for (const off of offs) off()
  }
}

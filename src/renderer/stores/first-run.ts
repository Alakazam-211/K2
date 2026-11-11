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

/** Row ids this client knows, in the contract's order. The daemon may send
 *  more (`meet_k5` later); unknown ids are dropped. */
export const FIRST_RUN_ROW_IDS = ['llm_sign_in', 'first_agent', 'tunnel', 'phone', 'own_sign_in'] as const
export type FirstRunRowId = (typeof FIRST_RUN_ROW_IDS)[number]

/** What a row's button opens. Unknown values are dropped (the row stays,
 *  without an opener). */
export const FIRST_RUN_OPENS = ['llm_sign_in', 'add_workspace', 'tunnel', 'phone', 'owner_account_prompt'] as const
export type FirstRunOpens = (typeof FIRST_RUN_OPENS)[number]

export type FirstRunRowState = 'done' | 'todo' | 'skipped'
export type FirstRunNote = 'needs_tunnel'

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
}

export interface FirstRunEntry {
  doc: FirstRunDoc | null
  error: string | null
}

function isOneOf<T extends string>(list: readonly T[], v: unknown): v is T {
  return typeof v === 'string' && (list as readonly string[]).includes(v)
}

/** Parse the route's body defensively: unknown row ids, `opens` values and
 *  notes are dropped, an unknown state reads as `todo`, and a malformed
 *  body is a switched-off checklist (never a crash). */
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
      note: row.note === 'needs_tunnel' ? 'needs_tunnel' : null,
    })
  }
  return {
    enabled: o.enabled === true,
    hidden: o.hidden === true,
    finished: o.finished === true,
    rows,
  }
}

/** The rows to draw, or null when the checklist shows nothing: the switch
 *  is off, the person hid it, it's finished, or no row is left to show. */
export function visibleRows(doc: FirstRunDoc | null | undefined): FirstRunRow[] | null {
  if (!doc || !doc.enabled || doc.hidden || doc.finished || doc.rows.length === 0) return null
  return doc.rows
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

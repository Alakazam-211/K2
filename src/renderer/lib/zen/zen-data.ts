// prd-zen-mode-v1 S6 (Z34–Z43, vs-live Z52, Z54, Z65, Z67; Rosson's
// 2026-10-04 answers 4–6, 9, 10) — what the v1 data verbs do.
//
// Every Zen widget, the built-in Agents and Conversation included, reads and
// writes K2 only through the bridge (`zen-bridge.ts`). This module is the K2
// side of the data verbs: `installZenDataVerbs()` registers them with
// `registerZenVerb`. A widget never gets a scope, a token or `daemonCli*`.
//
// Rows (`agents.list` / `agents.subscribe`): the selected Home's rows in
// Home order (so ⌘1–9 stays meaningful), multi-server, from the stores Home
// already keeps:
//   - live status (Z39, Z65), first source that knows: an open room's own
//     activity (its `agent_status_changed` slice), then `active-agents` for a
//     row on the window's server, then the pool's `presence/summary`
//     `agentActivity`. A server that sends no activity (before 0.43.2) shows
//     no status, never "idle" (Z71);
//   - availability from the pool (offline, sign in, no access) with Home's
//     own row rules (`computeRowStatus`), plus "Update <server>" below the
//     room floor;
//   - a last-message preview (Z41): one `GET /cli/thread/latest` per server
//     every 15 s while a Zen window shows rows, on start and on window focus;
//     a server without `thread-latest` gets one `thread?addr=&limit=1` per
//     row (never `limit=0`). The open conversation updates its row at once.
//   - No unread tracking (answer 9): `thread.markRead` is not provided.
//
// Conversations (`conversation.open`): Zen NEVER switches the window's server
// (Z35, vs-live Z54). A row on the window's server selects the primary room;
// any other row opens its pinned room in place (`homeRooms.open`) whatever
// "Open agents from other servers here" says; a server below the floor shows
// "Update <server> to message this agent here". The Thread address is
// resolved like the Agents page resolves it (`resolvePinnedChatCopyableAddress`,
// vs-live Z52), not the row handle.
//
// Threads (`thread.*`): the live view comes from the existing overlay Thread
// hook (`useOverlayThread`), mounted per subscribed conversation by
// `ZenDataHost`; posts go through the one compose send
// (`postThreadCompose`, vs-live Z67) after the existing attach path
// (`composeAttachPayload`: local paths, or an upload to the agent's own
// server).

import { createStore } from 'zustand/vanilla'
import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { usePageViewStore } from '@/stores/page-view'
import { useActiveAgentsStore, mergePaneStatus, type PaneStatus } from '@/stores/active-agents'
import { usePresenceStore, usersForWorkspace } from '@/stores/presence'
import { useWindowFocusStore } from '@/stores/window-focus'
import { homeRooms, type HomeRoomEntry } from '@/stores/home-rooms'
import { primaryRoom } from '@/stores/room'
import { hostPool } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { daemonCliGet } from '@/lib/daemon-cli'
import {
  LOCAL_HOME_HOST,
  activeHomeHostKey,
  findWorkspaceForRow,
  parseHomeAddress,
  savedHostForKey,
} from '@/lib/home-address'
import { computeRowStatus } from '@/components/Home/home-room'
import { roomRowActivity } from '@/lib/home-status'
import { homeRoomVerdict } from '@/lib/home-room-floor'
import { resolvePinnedChatCopyableAddress } from '@/lib/chat-session-tab'
import { composeAttachPayload } from '@/lib/compose-attach'
import {
  OVERLAY_PAGE_SIZE,
  isVoidedHitl,
  postThreadCompose,
  threadItemsFromSnapshot,
  type OverlayDoc,
  type OverlayThreadItem,
} from '@/components/SessionView/overlayThread'
import { registerZenVerb } from './zen-bridge'
import { exitZen, registerZenRowSelect } from './zen-view'

// ── Shapes a widget sees ──────────────────────────────────────────────────

/** What the agent is doing right now (answer 9). */
export type ZenActivity = 'working' | 'needs-you' | 'idle'

/** Whether the row can be messaged, and if not, why. */
export type ZenRowState =
  | 'ok'
  | 'checking'
  | 'starting'
  | 'offline'
  | 'signing-in'
  | 'sign-in'
  | 'no-access'
  | 'not-found'
  | 'update'

export interface ZenPreview {
  /** One line, at most 140 characters. */
  text: string
  /** Seconds since the epoch, or null. */
  at: number | null
  from: string
  /** The user's own message (`via=compose`). */
  mine: boolean
  seq: number | null
}

export interface ZenPerson {
  user: string
  name: string
}

export interface ZenAgentRow {
  /** The Home row address, `handle::host`. Every verb takes it. */
  address: string
  label: string
  /** Position in the Home (⌘1–9 selects index 0–8). */
  index: number
  hostKey: string
  /** The server's name when the agent isn't on this computer, else null. */
  server: string | null
  /** Your role on that server, when the pool has read it. */
  role: string | null
  reach: 'unknown' | 'live' | 'starting' | 'offline'
  auth: string | null
  /** Live status; null when the server can't say (no indicator). */
  activity: ZenActivity | null
  working: boolean
  needsYou: boolean
  state: ZenRowState
  /** Short state text when not `ok` ("Offline", "Sign in", …). */
  stateLabel: string | null
  /** Why, in a sentence (the conversation shows it). */
  detail: string | null
  preview: ZenPreview | null
  /** Other people on this agent right now (never you). */
  people: ZenPerson[]
  selected: boolean
  /** Selecting it opens a conversation that can be messaged. */
  openable: boolean
}

export type ZenConversationPhase = 'opening' | 'ready' | 'unavailable' | 'failed'

/** The live Thread a conversation shows (`thread.subscribe`). */
export interface ZenThreadView {
  address: string
  phase: ZenConversationPhase | 'closed'
  /** Copy for anything but `ready` (and for a refused login while ready). */
  note: string | null
  /** Visible items, oldest first (voided cards dropped). */
  items: OverlayThreadItem[]
  loaded: boolean
  hasMore: boolean
  loadingOlder: boolean
  error: string | null
}

/** `thread.post` options: files on this computer, or browser files. */
export interface ZenPostOptions {
  paths?: string[]
  files?: File[]
}

// ── Internal state ─────────────────────────────────────────────────────────

interface Conversation {
  address: string
  phase: ZenConversationPhase
  note: string | null
  hostKey: string
  /** The agent's own server: the primary room's scope on the window's
   *  server, else `scopeForHost(hostKey)`. Null until ready. */
  scope: ServerScope | null
  threadAddr: string | null
  workspacePath: string
  generation: number
}

/** One Thread feed `ZenDataHost` mounts (`useOverlayThread`). */
export interface ZenFeedSpec {
  address: string
  scope: ServerScope
  threadAddr: string
  /** Bumped when the conversation is re-resolved: remounts the feed. */
  generation: number
}

export interface ZenFeedView {
  items: OverlayThreadItem[]
  conversationId: string
  loaded: boolean
  hasMore: boolean
  loadingOlder: boolean
  error: string | null
}

export interface ZenFeedHandlers {
  loadOlder(): Promise<void>
  answer(id: string, payload: { answer?: string; secret?: string }): Promise<void>
  voidCard(id: string): Promise<void>
}

/** The feeds `ZenDataHost` renders. */
export const zenThreadFeeds = createStore<{ feeds: ZenFeedSpec[] }>(() => ({ feeds: [] }))

/** Preview refresh while a Zen window shows rows (Z41). */
export const ZEN_PREVIEW_MS = 15_000
/** `thread/latest` takes at most this many addrs (Z41). */
export const ZEN_LATEST_MAX = 50
const PREVIEW_CHARS = 140

const previews = new Map<string, ZenPreview>()
const selection = new Map<string, string>()
const conversations = new Map<string, Conversation>()
/** The newest `conversation.open` per address (older ones give way). */
const latestOpen = new Map<string, number>()
const feedViews = new Map<string, ZenFeedView>()
const feedHandlers = new Map<string, ZenFeedHandlers>()
const rowListeners = new Set<(rows: ZenAgentRow[]) => void>()
const threadListeners = new Map<string, Set<(view: ZenThreadView) => void>>()
let generation = 0

// ── Parsing at the boundary ────────────────────────────────────────────────

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** One preview line from a Thread doc, the server's rule (Z41) for servers
 *  without `thread-latest`: a choice card is "Asked: <prompt>", a secret
 *  card "Asked for a secret". */
export function previewText(doc: Pick<OverlayDoc, 'kind' | 'body' | 'choice'>): string {
  const raw =
    doc.kind === 'choice'
      ? `Asked: ${doc.choice?.prompt || doc.body || ''}`
      : doc.kind === 'secret'
        ? 'Asked for a secret'
        : doc.body ?? ''
  const one = raw.split(/\s+/).filter(Boolean).join(' ')
  if ([...one].length <= PREVIEW_CHARS) return one
  return `${[...one].slice(0, PREVIEW_CHARS - 1).join('').trimEnd()}…`
}

export interface ThreadLatestItem {
  addr: string
  conversationId: string | null
  seq: number | null
  at: number | null
  from: string | null
  via: string | null
  kind: string | null
  preview: string | null
  error: string | null
}

/** Parse `GET /cli/thread/latest`. Throws on a body that isn't one. */
export function parseThreadLatest(raw: unknown): ThreadLatestItem[] {
  if (!isObj(raw) || !Array.isArray(raw.items)) throw new Error('thread/latest: no items in the answer')
  const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)
  const str = (v: unknown): string | null => (typeof v === 'string' ? v : null)
  const out: ThreadLatestItem[] = []
  for (const it of raw.items) {
    if (!isObj(it) || typeof it.addr !== 'string') continue
    const err = isObj(it.error) ? (str(it.error.hint) ?? str(it.error.code) ?? 'error') : str(it.error)
    out.push({
      addr: it.addr,
      conversationId: str(it.conversationId),
      seq: num(it.seq),
      at: num(it.at),
      from: str(it.from),
      via: str(it.via),
      kind: str(it.kind),
      preview: str(it.preview),
      error: err,
    })
  }
  return out
}

function previewFromItem(item: OverlayThreadItem): ZenPreview {
  return {
    text: previewText(item.doc),
    at: typeof item.doc.created_at === 'number' ? item.doc.created_at : null,
    from: item.doc.from || '',
    mine: item.doc.via === 'compose',
    seq: Number.isFinite(item.seq) ? item.seq : null,
  }
}

function setPreview(address: string, next: ZenPreview): boolean {
  const prev = previews.get(address)
  if (prev && prev.seq !== null && next.seq !== null && next.seq < prev.seq) return false
  if (prev && prev.text === next.text && prev.seq === next.seq && prev.at === next.at) return false
  previews.set(address, next)
  return true
}

// ── Rows ───────────────────────────────────────────────────────────────────

function serverLabel(hostKey: string, hosts: ConnectHost[]): string | null {
  if (hostKey === LOCAL_HOME_HOST) return null
  const saved = savedHostForKey(hosts, hostKey)
  return saved ? saved.label || saved.hostname : hostKey || 'unknown server'
}

function fromPane(s: PaneStatus): ZenActivity {
  if (s === 'permission') return 'needs-you'
  if (s === 'working') return 'working'
  return 'idle'
}

const STATE_LABEL: Record<Exclude<ZenRowState, 'ok' | 'update'>, string> = {
  checking: 'Checking…',
  starting: 'Starting',
  offline: 'Offline',
  'signing-in': 'Signing in…',
  'sign-in': 'Sign in',
  'no-access': 'No access',
  'not-found': 'Not found',
}

function roomActivityOf(entry: HomeRoomEntry | undefined): ReturnType<typeof roomRowActivity> | null {
  if (!entry || entry.phase !== 'open' || !entry.room) return null
  return roomRowActivity(entry.room.activityView.getState(), mergePaneStatus)
}

function rowFor(row: HomeRow, index: number, home: Home): ZenAgentRow {
  const host = useConnectHostStore.getState()
  const projects = useProjectsStore.getState().projects
  const parsed = parseHomeAddress(row.address)
  const hostKey = parsed?.host ?? ''
  const connectedKey = activeHomeHostKey(host.activeHost)
  const entry: HostEntry | undefined = hostPool.store.getState().entries[hostKey]
  const roomEntry = homeRooms.store.getState().entries[row.address]
  const roomActivity = roomActivityOf(roomEntry)
  const { status, onConnected } = computeRowStatus(row, {
    activeHost: host.activeHost,
    hosts: host.hosts,
    connectionStatus: host.connectionStatus,
    projects,
    entry,
    sameServerAs: null,
    roomActivity,
  })
  const server = serverLabel(hostKey, host.hosts)
  let state: ZenRowState
  let activity: ZenActivity | null = null
  let people: ZenPerson[] = []
  let detail: string | null = status.detail ?? null
  if (onConnected) {
    if (status.kind === 'idle') {
      state = 'ok'
      const ws = findWorkspaceForRow(projects, row)
      if (ws) {
        activity = fromPane(useActiveAgentsStore.getState().getProjectStatus(ws.id))
        const presence = usePresenceStore.getState()
        const self = host.activeHost === 'local' ? 'owner' : host.activeHost.username || 'owner'
        people = presence.supported
          ? usersForWorkspace(presence.roster, ws.path)
              .filter((u) => u.user !== self)
              .map((u) => ({ user: u.user, name: u.user }))
          : []
      }
    } else {
      state = status.kind === 'not-found' ? 'not-found' : 'checking'
    }
  } else {
    switch (status.kind) {
      case 'working':
      case 'permission':
      case 'live':
      case 'idle':
      case 'review':
        state = 'ok'
        activity =
          status.kind === 'working'
            ? 'working'
            : status.kind === 'permission'
              ? 'needs-you'
              : roomActivity !== null || Array.isArray(entry?.activity)
                ? 'idle'
                : null
        break
      default:
        state = status.kind
    }
    people = status.people.map((p) => ({ user: p.user, name: p.name }))
    // Z35: below the room floor Zen shows the Update copy (never switches).
    if (state === 'ok' && hostKey !== LOCAL_HOME_HOST && homeRoomVerdict(entry?.boot) === 'switch') {
      state = 'update'
      activity = null
    }
  }
  const where = server ?? 'this computer'
  const stateLabel = state === 'ok' ? null : state === 'update' ? `Update ${where}` : STATE_LABEL[state]
  if (state === 'update') detail = `Update ${where} to message this agent here.`
  else if (state === 'no-access') detail = `Your login on ${where} can’t message agents.${detail ? ` ${detail}` : ''}`
  else if (state === 'offline') detail = detail ?? `${where} is offline.`
  else if (state === 'sign-in') detail = detail ?? `Sign in to ${where} to message ${row.label}.`
  else if (state === 'not-found') detail = `${row.label} is not on ${where} any more.`
  else if (state === 'ok') detail = null
  return {
    address: row.address,
    label: row.label,
    index,
    hostKey,
    server,
    role: onConnected ? null : (entry?.role ?? null),
    reach: onConnected ? (host.connectionStatus === 'connected' ? 'live' : 'unknown') : (entry?.reach ?? 'unknown'),
    auth: onConnected ? 'ok' : (entry?.auth ?? null),
    activity,
    working: activity === 'working',
    needsYou: activity === 'needs-you',
    state,
    stateLabel,
    detail,
    preview: previews.get(row.address) ?? null,
    people,
    selected: selection.get(home.id) === row.address,
    openable: state === 'ok',
  }
}

/** `agents.list()`: the selected Home's rows, in Home order. */
export function zenAgentRows(): ZenAgentRow[] {
  const home = selectedHome(useHomesStore.getState())
  return home.rows.map((r, i) => rowFor(r, i, home))
}

let lastRowsSig = ''
let lastRows: ZenAgentRow[] = []

function emitRows(): void {
  if (rowListeners.size === 0) return
  const rows = zenAgentRows()
  const sig = JSON.stringify(rows)
  if (sig === lastRowsSig) return
  lastRowsSig = sig
  lastRows = rows
  for (const fn of [...rowListeners]) fn(rows)
}

// ── Live: store subscriptions + the preview poll ──────────────────────────

let stopLive: (() => void) | null = null

function startLive(): void {
  if (stopLive) return
  const unsubs: Array<() => void> = []
  const on = (): void => emitRows()
  unsubs.push(useHomesStore.subscribe(on))
  unsubs.push(useConnectHostStore.subscribe(on))
  unsubs.push(useProjectsStore.subscribe(on))
  unsubs.push(useActiveAgentsStore.subscribe(on))
  unsubs.push(usePresenceStore.subscribe(on))
  unsubs.push(hostPool.store.subscribe(on))
  // Open rooms' own activity slices (Z39): re-subscribe when rooms change.
  const roomSubs = new Map<string, () => void>()
  const syncRooms = (): void => {
    const entries = homeRooms.store.getState().entries
    for (const [address, unsub] of roomSubs) {
      const e = entries[address]
      if (!e || !e.room) {
        unsub()
        roomSubs.delete(address)
      }
    }
    for (const e of Object.values(entries)) {
      if (e.room && !roomSubs.has(e.address)) roomSubs.set(e.address, e.room.activityView.subscribe(on))
    }
    on()
  }
  syncRooms()
  unsubs.push(homeRooms.store.subscribe(syncRooms))
  unsubs.push(() => {
    for (const u of roomSubs.values()) u()
    roomSubs.clear()
  })
  // Z41: previews now, every 15 s, and when the window gains focus. Z37:
  // focus with a remote conversation shown is that room's `focused()`.
  void refreshZenPreviews()
  const timer = setInterval(() => void refreshZenPreviews(), ZEN_PREVIEW_MS)
  unsubs.push(() => clearInterval(timer))
  unsubs.push(
    useWindowFocusStore.subscribe((s, prev) => {
      if (!s.isFocused || prev.isFocused) return
      void refreshZenPreviews()
      const home = selectedHome(useHomesStore.getState())
      const address = selection.get(home.id)
      const conv = address ? conversations.get(address) : undefined
      if (address && conv && conv.phase === 'ready' && !isConnectedHost(conv.hostKey)) homeRooms.focused(address)
    }),
  )
  stopLive = () => {
    for (const u of unsubs) u()
    stopLive = null
  }
}

function isConnectedHost(hostKey: string): boolean {
  return hostKey === activeHomeHostKey(useConnectHostStore.getState().activeHost)
}

/** The agent's own server for a row (Z35): the primary room on the
 *  window's server, else the pinned scope. */
function scopeForRow(hostKey: string): ServerScope {
  return isConnectedHost(hostKey) ? primaryRoom().scope : scopeForHost(hostKey)
}

const previewInFlight = new Set<string>()

/** One preview refresh: one request per server (Z41). */
export async function refreshZenPreviews(): Promise<void> {
  if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
  const rows = zenAgentRows().filter((r) => r.state === 'ok')
  const byHost = new Map<string, ZenAgentRow[]>()
  for (const r of rows) {
    const list = byHost.get(r.hostKey) ?? []
    list.push(r)
    byHost.set(r.hostKey, list)
  }
  await Promise.all(
    [...byHost].map(async ([hostKey, hostRows]) => {
      if (previewInFlight.has(hostKey)) return
      previewInFlight.add(hostKey)
      try {
        await refreshHostPreviews(hostKey, hostRows)
      } catch (err) {
        console.warn(`[zen] previews from ${hostKey} failed:`, err)
      } finally {
        previewInFlight.delete(hostKey)
      }
    }),
  )
  emitRows()
}

async function refreshHostPreviews(hostKey: string, rows: ZenAgentRow[]): Promise<void> {
  const scope = scopeForRow(hostKey)
  const handleOf = (r: ZenAgentRow): string => parseHomeAddress(r.address)?.handle ?? ''
  if (scope.serverSupports('thread-latest')) {
    for (let i = 0; i < rows.length; i += ZEN_LATEST_MAX) {
      const chunk = rows.slice(i, i + ZEN_LATEST_MAX)
      const raw = await daemonCliGet<unknown>(scope, 'thread/latest', { addrs: chunk.map(handleOf).join(',') })
      const items = parseThreadLatest(raw)
      for (const r of chunk) {
        const it = items.find((x) => x.addr === handleOf(r))
        if (!it || it.error || it.preview === null) continue
        setPreview(r.address, {
          text: it.preview,
          at: it.at,
          from: it.from ?? '',
          mine: it.via === 'compose',
          seq: it.seq,
        })
      }
    }
    return
  }
  // Older server: one newest item per row (never limit=0, Z41).
  await Promise.all(
    rows.map(async (r) => {
      const raw = await daemonCliGet<unknown>(scope, 'thread', { addr: handleOf(r), limit: 1 })
      const snap = threadItemsFromSnapshot(raw)
      const last = snap.items[snap.items.length - 1]
      if (last) setPreview(r.address, previewFromItem(last))
    }),
  )
}

// ── Conversations ──────────────────────────────────────────────────────────

function findRow(address: string): { home: Home; row: HomeRow; index: number } {
  const home = selectedHome(useHomesStore.getState())
  const index = home.rows.findIndex((r) => r.address === address)
  if (index < 0) throw new Error(`zen: ${address} is not a row of ${home.name}`)
  return { home, row: home.rows[index], index }
}

function setConversation(c: Conversation): void {
  conversations.set(c.address, c)
  syncFeeds()
  emitThread(c.address)
  emitRows()
}

/** Feeds = ready conversations someone subscribed to. */
function syncFeeds(): void {
  const feeds: ZenFeedSpec[] = []
  for (const [address, subs] of threadListeners) {
    if (subs.size === 0) continue
    const c = conversations.get(address)
    if (!c || c.phase !== 'ready' || !c.scope || !c.threadAddr) continue
    feeds.push({ address, scope: c.scope, threadAddr: c.threadAddr, generation: c.generation })
  }
  const prev = zenThreadFeeds.getState().feeds
  const same =
    prev.length === feeds.length &&
    prev.every((p, i) => p.address === feeds[i].address && p.generation === feeds[i].generation)
  if (!same) zenThreadFeeds.setState({ feeds })
}

/** `conversation.open(address, {where?})` (Z35, Z37, Z43). */
export async function openZenConversation(
  address: string,
  opts: { where?: 'zen' | 'agents' } = {},
): Promise<ZenAgentRow> {
  const { home, row, index } = findRow(address)
  if (opts.where === 'agents') {
    await openInAgents(row)
    return rowFor(row, index, home)
  }
  selection.set(home.id, address)
  const current = rowFor(row, index, home)
  const gen = ++generation
  const base = { address, hostKey: current.hostKey, scope: null, threadAddr: null, workspacePath: '', generation: gen }
  if (current.state !== 'ok' && current.state !== 'checking') {
    setConversation({ ...base, phase: 'unavailable', note: current.detail ?? current.stateLabel })
    return rowFor(row, index, home)
  }
  const existing = conversations.get(address)
  latestOpen.set(address, gen)
  const stale = (): boolean => latestOpen.get(address) !== gen
  const where = current.server ?? 'this computer'
  // The row shows as selected at once; its Thread follows.
  if (existing && existing.phase === 'ready') emitRows()
  else {
    setConversation({
      ...base,
      phase: 'opening',
      note: isConnectedHost(current.hostKey) ? `Opening ${row.label}…` : `Connecting to ${where}…`,
    })
  }

  let scope: ServerScope
  let workspacePath: string
  let projectId: string | null
  if (isConnectedHost(current.hostKey)) {
    const ws = findWorkspaceForRow(useProjectsStore.getState().projects, row)
    if (!ws) {
      setConversation({ ...base, phase: 'unavailable', note: `${row.label} is not on ${where} any more.` })
      return rowFor(row, index, home)
    }
    // MS2: the window's own room (no window switch).
    homeRooms.showPrimary()
    useProjectsStore.getState().setActiveProject(ws.id)
    scope = primaryRoom().scope
    workspacePath = ws.path
    projectId = ws.id
  } else {
    // Know the server's version before the room is built, so a server
    // below the floor never reaches the room's own switch (Z35).
    if (!hostPool.entry(current.hostKey)?.boot?.version) await hostPool.check(current.hostKey)
    if (homeRoomVerdict(hostPool.entry(current.hostKey)?.boot) === 'switch') {
      setConversation({ ...base, phase: 'unavailable', note: `Update ${where} to message this agent here.` })
      return rowFor(row, index, home)
    }
    // In place, whatever "Open agents from other servers here" says.
    const entry = await homeRooms.open(row, current.hostKey)
    if (stale()) return rowFor(row, index, home)
    if (entry.phase !== 'open' || !entry.room) {
      const note =
        entry.phase === 'not-found'
          ? `${row.label} is not on ${where} any more.`
          : entry.phase === 'switched'
            ? `Update ${where} to message this agent here.`
            : `Couldn’t open ${row.label}${entry.error ? `: ${entry.error}` : '.'}`
      setConversation({ ...base, phase: 'failed', note })
      return rowFor(row, index, home)
    }
    scope = scopeForHost(current.hostKey)
    workspacePath = entry.room.cwd()
    projectId = entry.room.activeProjectId()
  }
  if (existing && existing.phase === 'ready' && existing.scope === scope && existing.workspacePath === workspacePath) {
    // Already resolved: keep the feed (same generation), just re-show.
    syncFeeds()
    emitRows()
    return rowFor(row, index, home)
  }
  // vs-live Z52: the Agents page's own resolver, not the row handle.
  const resolved = await resolvePinnedChatCopyableAddress(scope, workspacePath, projectId)
  if (stale()) return rowFor(row, index, home)
  const fallback = parseHomeAddress(address)?.handle ?? ''
  if (!resolved?.clipboard) console.warn(`[zen] no pinned Chat address for ${address}; using ${fallback}`)
  setConversation({
    ...base,
    phase: 'ready',
    note: null,
    scope,
    threadAddr: resolved?.clipboard || fallback,
    workspacePath,
  })
  return rowFor(row, index, home)
}

/** "Open in Agents" (Z43, answer 10): leave the Zen view for the agent's
 *  terminal, where its permission prompt is. A row on the window's server
 *  opens on the Agents page (Zen stays on for the Home). A row on another
 *  server only has a room on Home, so Zen turns off for this Home and Home
 *  shows that room, in place. The window's server never changes. */
async function openInAgents(row: HomeRow): Promise<void> {
  const hostKey = parseHomeAddress(row.address)?.host ?? ''
  if (isConnectedHost(hostKey)) {
    const ws = findWorkspaceForRow(useProjectsStore.getState().projects, row)
    if (ws) useProjectsStore.getState().setActiveProject(ws.id)
    homeRooms.showPrimary()
    usePageViewStore.getState().setPage('agents')
    return
  }
  if (!hostPool.entry(hostKey)?.boot?.version) await hostPool.check(hostKey)
  exitZen()
  usePageViewStore.getState().setPage('home')
  if (homeRoomVerdict(hostPool.entry(hostKey)?.boot) === 'switch') return
  await homeRooms.open(row, hostKey)
}

function closeZenConversation(): void {
  const home = selectedHome(useHomesStore.getState())
  selection.delete(home.id)
  emitRows()
}

// ── Threads ────────────────────────────────────────────────────────────────

const ROLE_REFUSED = /role_required/

function threadView(address: string): ZenThreadView {
  const c = conversations.get(address)
  const f = feedViews.get(address)
  const items = (f?.items ?? []).filter((it) => !isVoidedHitl(it.doc))
  let note = c?.note ?? null
  if (c?.phase === 'ready' && f?.error && ROLE_REFUSED.test(f.error)) {
    const server = serverLabel(c.hostKey, useConnectHostStore.getState().hosts) ?? 'this computer'
    note = `Your login on ${server} can’t message agents.`
  }
  return {
    address,
    phase: c ? c.phase : 'closed',
    note,
    items,
    loaded: f?.loaded ?? false,
    hasMore: f?.hasMore ?? false,
    loadingOlder: f?.loadingOlder ?? false,
    error: f?.error ?? null,
  }
}

function emitThread(address: string): void {
  const subs = threadListeners.get(address)
  if (!subs || subs.size === 0) return
  const view = threadView(address)
  for (const fn of [...subs]) fn(view)
}

/** `ZenDataHost` publishes one feed's state (from `useOverlayThread`). */
export function publishZenFeed(address: string, view: ZenFeedView, handlers: ZenFeedHandlers): void {
  feedViews.set(address, view)
  feedHandlers.set(address, handlers)
  const visible = view.items.filter((it) => !isVoidedHitl(it.doc))
  const last = visible[visible.length - 1]
  const changed = last ? setPreview(address, previewFromItem(last)) : false
  emitThread(address)
  if (changed) emitRows()
}

export function clearZenFeed(address: string): void {
  feedViews.delete(address)
  feedHandlers.delete(address)
}

function readyConversation(address: string): Conversation & { scope: ServerScope; threadAddr: string } {
  const c = conversations.get(address)
  if (!c || c.phase !== 'ready' || !c.scope || !c.threadAddr) {
    throw new Error(`zen: the conversation with ${address} isn't open`)
  }
  return c as Conversation & { scope: ServerScope; threadAddr: string }
}

/** `thread.post(address, text, {paths?, files?})`: the existing attach
 *  path (local paths as they are, or an upload to the agent's own server),
 *  then the one compose send. */
export async function postZenThread(
  address: string,
  text: string,
  opts: ZenPostOptions = {},
): Promise<{ id: string | null; seq: number | null }> {
  const c = readyConversation(address)
  let body = text.trim()
  const hasFiles = (opts.paths?.length ?? 0) > 0 || (opts.files?.length ?? 0) > 0
  if (hasFiles) {
    const payload = await composeAttachPayload(c.scope, {
      paths: opts.paths,
      files: opts.files,
      workspacePath: c.workspacePath,
    })
    if (!payload) throw new Error('The attachment was not uploaded.')
    body = body ? `${body}\n${payload.trim()}` : payload.trim()
  }
  if (!body) throw new Error('Nothing to send.')
  // Z37: typing or sending counts as input on a pinned room.
  if (!isConnectedHost(c.hostKey)) homeRooms.input(address)
  const result = await postThreadCompose(c.scope, c.threadAddr, body)
  if (!result.ok) throw new Error(result.error)
  if (result.item) {
    if (setPreview(address, previewFromItem(result.item))) emitRows()
  }
  return { id: result.item?.id ?? null, seq: result.item?.seq ?? null }
}

function handlersFor(address: string): ZenFeedHandlers {
  readyConversation(address)
  const h = feedHandlers.get(address)
  if (!h) throw new Error(`zen: the conversation with ${address} isn't on screen`)
  return h
}

// ── Verbs ──────────────────────────────────────────────────────────────────

function str(v: unknown, what: string): string {
  if (typeof v !== 'string' || !v.trim()) throw new Error(`zen bridge: ${what} must be a non-empty string`)
  return v
}

function fn<T>(v: unknown, what: string): (arg: T) => void {
  if (typeof v !== 'function') throw new Error(`zen bridge: ${what} must be a function`)
  return v as (arg: T) => void
}

function rowsSubscribe(cb: (rows: ZenAgentRow[]) => void): () => void {
  rowListeners.add(cb)
  const rows = zenAgentRows()
  lastRowsSig = JSON.stringify(rows)
  lastRows = rows
  startLive()
  cb(lastRows)
  return () => {
    rowListeners.delete(cb)
    if (rowListeners.size === 0) {
      stopLive?.()
      lastRowsSig = ''
    }
  }
}

function threadSubscribe(address: string, cb: (view: ZenThreadView) => void): () => void {
  let subs = threadListeners.get(address)
  if (!subs) {
    subs = new Set()
    threadListeners.set(address, subs)
  }
  subs.add(cb)
  syncFeeds()
  cb(threadView(address))
  return () => {
    const s = threadListeners.get(address)
    s?.delete(cb)
    if (s && s.size === 0) threadListeners.delete(address)
    syncFeeds()
  }
}

/** Register the v1 data verbs and the ⌘1–9 row select. Returns the
 *  uninstall. */
export function installZenDataVerbs(): () => void {
  const offs = [
    registerZenVerb('agents.list', () => zenAgentRows()),
    registerZenVerb('agents.subscribe', (_ctx, cb) => rowsSubscribe(fn<ZenAgentRow[]>(cb, 'agents.subscribe callback'))),
    registerZenVerb('presence.get', (_ctx, address) => {
      const a = str(address, 'address')
      return zenAgentRows().find((r) => r.address === a)?.people ?? []
    }),
    registerZenVerb('presence.subscribe', (_ctx, address, cb) => {
      const a = str(address, 'address')
      const f = fn<ZenPerson[]>(cb, 'presence.subscribe callback')
      let last = ''
      return rowsSubscribe((rows) => {
        const people = rows.find((r) => r.address === a)?.people ?? []
        const sig = JSON.stringify(people)
        if (sig === last) return
        last = sig
        f(people)
      })
    }),
    registerZenVerb('conversation.open', (_ctx, address, opts) =>
      openZenConversation(
        str(address, 'address'),
        isObj(opts) && (opts.where === 'agents' || opts.where === 'zen') ? { where: opts.where } : {},
      ),
    ),
    registerZenVerb('conversation.close', () => closeZenConversation()),
    registerZenVerb('thread.subscribe', (_ctx, address, cb) =>
      threadSubscribe(str(address, 'address'), fn<ZenThreadView>(cb, 'thread.subscribe callback')),
    ),
    registerZenVerb('thread.read', async (_ctx, address, opts) => {
      const a = str(address, 'address')
      const o = isObj(opts) ? opts : {}
      const c = readyConversation(a)
      if (typeof o.beforeSeq === 'number') {
        // Older items join the live feed (the overlay hook's own paging).
        await handlersFor(a).loadOlder()
        return threadView(a)
      }
      const limit = Math.min(100, Math.max(1, typeof o.limit === 'number' ? Math.floor(o.limit) : OVERLAY_PAGE_SIZE))
      const snap = threadItemsFromSnapshot(await daemonCliGet<unknown>(c.scope, 'thread', { addr: c.threadAddr, limit }))
      return {
        conversationId: snap.conversation_id,
        items: snap.items.filter((it) => !isVoidedHitl(it.doc)),
        hasMore: snap.has_more,
      }
    }),
    registerZenVerb('thread.post', (_ctx, address, text, opts) => {
      const o = isObj(opts) ? opts : {}
      const paths = Array.isArray(o.paths) ? o.paths.filter((p): p is string => typeof p === 'string') : undefined
      const files =
        Array.isArray(o.files) && typeof File !== 'undefined'
          ? o.files.filter((f): f is File => f instanceof File)
          : undefined
      return postZenThread(str(address, 'address'), typeof text === 'string' ? text : '', { paths, files })
    }),
    registerZenVerb('thread.answer', (_ctx, address, cardId, answer) => {
      const payload =
        typeof answer === 'string'
          ? { answer }
          : isObj(answer)
            ? {
                ...(typeof answer.answer === 'string' ? { answer: answer.answer } : {}),
                ...(typeof answer.secret === 'string' ? { secret: answer.secret } : {}),
              }
            : null
      if (!payload || (payload.answer === undefined && payload.secret === undefined)) {
        throw new Error('zen bridge: thread.answer needs an answer or a secret')
      }
      return handlersFor(str(address, 'address')).answer(str(cardId, 'cardId'), payload)
    }),
    registerZenVerb('thread.void', (_ctx, address, cardId) =>
      handlersFor(str(address, 'address')).voidCard(str(cardId, 'cardId')),
    ),
    // Z32/Z54: ⌘1–9 in Zen selects conversation N, in place.
    registerZenRowSelect((index) => {
      const row = selectedHome(useHomesStore.getState()).rows[index]
      if (!row) return
      void openZenConversation(row.address).catch((err: unknown) => console.warn('[zen] open failed:', err))
    }),
  ]
  return () => {
    for (const off of offs) off()
  }
}

/** Tests only. */
export function __resetZenDataForTests(): void {
  stopLive?.()
  previews.clear()
  selection.clear()
  conversations.clear()
  latestOpen.clear()
  feedViews.clear()
  feedHandlers.clear()
  rowListeners.clear()
  threadListeners.clear()
  previewInFlight.clear()
  lastRowsSig = ''
  lastRows = []
  zenThreadFeeds.setState({ feeds: [] })
}

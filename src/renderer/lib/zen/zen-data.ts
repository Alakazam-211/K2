// prd-zen-mode-v1 S6 (Z34–Z43, vs-live Z52, Z54, Z65, Z67; Rosson's
// 2026-10-04 answers 4–6, 9, 10) and prd-zen-gardens-v1 G26–G28, G32,
// G52–G54, G61 — what the data verbs do.
//
// Every Zen widget, the built-in Agents and Conversation included, reads and
// writes K2 only through the bridge (`zen-bridge.ts`). This module is the K2
// side of the data verbs: `installZenDataVerbs()` registers them with
// `registerZenVerb`. A widget never gets a scope, a token or `daemonCli*`.
//
// The Agents view (Rosson 2026-10-04, `source: "workspaces"`): an Agents
// widget that lists THIS server's workspaces (the window's server) the way
// the app's Agents page does — pinned first, then the active focus group's
// workspaces plus ungrouped ones when focus groups are on (GH #26), else all
// of them. Its focus-group dropdown (`focusGroups.*`) reads and sets the
// app's own active focus group, without switching workspaces.
//
// Views (G27): each Agents widget in a Garden shows ONE Home — its own pick
// (`k2.zen.gardenHomes.v1`), else its `home` prop, else the Garden's
// `seedHome`, else the window's selected Home at that moment (then kept as
// its pick). Never the window's selected Home live: Zen never moves the
// Home page. With an `agent` prop it shows that one agent, filtered from
// its Home (Rosson's answer 5). A Conversation widget follows the first
// Agents widget, or the one its `agents` prop names. The empty-Garden
// widget's "Ask my agent" opens a conversation with an agent on this
// computer that needn't be on any Home. Selection is per view:
// `<gardenId>/<widgetId>`.
//
// Rows (`agents.list` / `agents.subscribe`): the view's rows in Home order
// (so ⌘1–9 stays meaningful), multi-server, from the stores Home already
// keeps:
//   - live status (Z39, Z65), first source that knows: an open room's own
//     activity (its `agent_status_changed` slice), then `active-agents` for a
//     row on the window's server, then the pool's `presence/summary`
//     `agentActivity`. A server that sends no activity (before 0.43.2) shows
//     no status, never "idle" (Z71). `ZenDataHost` runs Home's row-status
//     poll for every Home a view shows (G53);
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
// vs-live Z52), not the row handle. "Open in Agents" turns Zen off first
// (G32, G54).
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
import { useFocusGroupsStore } from '@/stores/focus-groups'
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
  homeAddress,
  parseHomeAddress,
  savedHostForKey,
  workspaceHandle,
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
import { registerZenVerb, type ZenVerbCtx } from './zen-bridge'
import { exitZen } from './zen-view'
import type { ZenResolvedPage, ZenWidgetDecl } from './zen-page'
import { useZenGardenHomesStore, zenGardenHomeKey } from './zen-garden-homes'
import { useZenGardensStore } from './zen-gardens'
import { draftZenCompose } from './zen-compose-drafts'
import { zenAgentsSource } from './zen-rail-views'

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

/** What one widget shows (G27): one Home's rows (optionally one agent of
 *  it), or — `homeId` null — only the agent picked in that widget (the
 *  empty Garden's Ask my agent). */
export interface ZenView {
  /** `<gardenId>/<widgetId>`: the selection key, and the Home pick key. */
  key: string
  gardenId: string
  /** The Agents widget the view belongs to (or the lone widget). */
  widgetId: string
  homeId: string | null
  /** Single-agent mode: a row address, handle or name (the `agent` prop). */
  agent: string | null
  /** `workspaces`: this server's workspaces (the Agents view), not a Home. */
  source?: 'home' | 'workspaces'
}

/** One focus group as the Agents view's dropdown shows it. */
export interface ZenFocusGroup {
  id: string
  name: string
  color: string | null
}

/** `focusGroups.get()`: the app's focus groups (Settings turns them on). */
export interface ZenFocusGroups {
  enabled: boolean
  groups: ZenFocusGroup[]
  /** The active group (null: none picked, or focus groups off). */
  active: string | null
}

/** The feeds `ZenDataHost` renders. */
export const zenThreadFeeds = createStore<{ feeds: ZenFeedSpec[] }>(() => ({ feeds: [] }))

/** The Homes the live views show: `ZenDataHost` runs Home's row-status
 *  poll for each (G53), since `HomeShellEffects` only runs on the Home page. */
export const zenViewHomes = createStore<{ homeIds: string[] }>(() => ({ homeIds: [] }))

/** Preview refresh while a Zen window shows rows (Z41). */
export const ZEN_PREVIEW_MS = 15_000
/** `thread/latest` takes at most this many addrs (Z41). */
export const ZEN_LATEST_MAX = 50
const PREVIEW_CHARS = 140

/** The template's own controls (Add agent) act for the page's first
 *  Agents widget. */
export const ZEN_TEMPLATE_CONTROLS_ID = 'template-controls'

const previews = new Map<string, ZenPreview>()
/** View key → the open conversation's address. */
const selection = new Map<string, string>()
const conversations = new Map<string, Conversation>()
/** The newest `conversation.open` per address (older ones give way). */
const latestOpen = new Map<string, number>()
const feedViews = new Map<string, ZenFeedView>()
const feedHandlers = new Map<string, ZenFeedHandlers>()
/** Agents on this computer that are on no Home (`agents.local`). */
const looseRows = new Map<string, HomeRow>()
interface RowListener {
  view(): ZenView | null
  cb(rows: ZenAgentRow[]): void
  sig: string
}
const rowListeners = new Set<RowListener>()
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

// ── Row status helpers ─────────────────────────────────────────────────────

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


// ── Views (G27) ────────────────────────────────────────────────────────────

function propString(props: Record<string, unknown>, ...names: string[]): string | null {
  for (const n of names) {
    const v = props[n]
    if (typeof v === 'string' && v.trim()) return v.trim()
  }
  return null
}

/** The Agents widget's `home-picker` prop (G11, G38). */
export function zenHomePickerOn(props: Record<string, unknown>): boolean {
  return props['home-picker'] === true || props.homePicker === true || props.home_picker === true
}

/** The page's first Agents widget: column order, then declaration order. */
export function firstAgentsWidget(page: ZenResolvedPage): ZenWidgetDecl | null {
  let best: ZenWidgetDecl | null = null
  for (const w of page.widgets) {
    if (w.kind !== 'agents') continue
    if (best === null || w.column < best.column) best = w
  }
  return best
}

/** A Home by id, or by name (case-insensitive). */
function findHome(homes: readonly Home[], ref: string | null): Home | null {
  if (!ref) return null
  const byId = homes.find((h) => h.id === ref)
  if (byId) return byId
  const lower = ref.toLocaleLowerCase()
  return homes.find((h) => h.name.toLocaleLowerCase() === lower) ?? null
}

/** The Home an Agents widget shows: its pick, its `home` prop, the
 *  Garden's `seedHome`, else the window's selected Home now (kept). */
export function zenHomeForWidget(gardenId: string, widget: ZenWidgetDecl): string {
  const homes = useHomesStore.getState().homes
  const key = zenGardenHomeKey(gardenId, widget.id)
  const picked = findHome(homes, useZenGardenHomesStore.getState().picks[key] ?? null)
  if (picked) return picked.id
  const prop = findHome(homes, propString(widget.props, 'home'))
  if (prop) return prop.id
  const seed = findHome(homes, useZenGardensStore.getState().gardens.find((g) => g.id === gardenId)?.seedHome ?? null)
  if (seed) return seed.id
  const now = selectedHome(useHomesStore.getState()).id
  // Keep it, so a later Home-page switch doesn't move this widget. Not
  // during a render: the next microtask.
  if (gardenId) queueMicrotask(() => useZenGardenHomesStore.getState().setPick(key, now))
  return now
}

/** The single agent a widget is filtered to (Rosson's answer 5): its
 *  `agent` prop, unless `mode` says the whole Home. The daemon sends every
 *  prop, `mode` derived (`"agent"` when `agent` is set). */
export function zenAgentFilter(props: Record<string, unknown>): string | null {
  if (props.mode === 'home') return null
  return propString(props, 'agent')
}

/** A widget's view of one Home (an Agents widget, or a Conversation pinned
 *  to one agent with `agent` + `home`). */
function homeView(gardenId: string, w: ZenWidgetDecl): ZenView {
  return {
    key: zenGardenHomeKey(gardenId, w.id),
    gardenId,
    widgetId: w.id,
    homeId: zenHomeForWidget(gardenId, w),
    agent: w.kind === 'conversation' ? propString(w.props, 'agent') : zenAgentFilter(w.props),
  }
}

function agentsView(gardenId: string, w: ZenWidgetDecl): ZenView {
  if (zenAgentsSource(w.props) === 'workspaces') {
    return {
      key: zenGardenHomeKey(gardenId, w.id),
      gardenId,
      widgetId: w.id,
      homeId: null,
      agent: zenAgentFilter(w.props),
      source: 'workspaces',
    }
  }
  return homeView(gardenId, w)
}

/** The app's focus groups, as the Agents view shows them. */
export function zenFocusGroups(): ZenFocusGroups {
  const fg = useFocusGroupsStore.getState()
  return {
    enabled: fg.focusGroupsEnabled,
    groups: fg.focusGroups.map((g) => ({ id: g.id, name: g.name, color: g.color })),
    active: fg.focusGroupsEnabled ? fg.activeFocusGroupId : null,
  }
}

/** This server's workspaces as rows, in the Agents page's order: pinned
 *  first, then the rest — with focus groups on, only the active group's and
 *  the ungrouped ones (GH #26: ungrouped show in every group). */
function workspaceRows(): HomeRow[] {
  const projects = useProjectsStore.getState().projects
  const fg = useFocusGroupsStore.getState()
  const hostKey = activeHomeHostKey(useConnectHostStore.getState().activeHost)
  const group = fg.focusGroupsEnabled ? fg.activeFocusGroupId : null
  const pinned = projects.filter((p) => p.pinned)
  const rest = projects.filter((p) => !p.pinned && (group === null || p.focusGroupId === group || p.focusGroupId == null))
  const out: HomeRow[] = []
  const seen = new Set<string>()
  for (const p of [...pinned, ...rest]) {
    const handle = workspaceHandle(p)
    if (!handle) continue
    const address = homeAddress(handle, hostKey)
    if (seen.has(address)) continue
    seen.add(address)
    out.push({ address, workspaceId: p.id, label: p.name })
  }
  return out
}

function looseView(gardenId: string, widgetId: string): ZenView {
  return { key: zenGardenHomeKey(gardenId, widgetId), gardenId, widgetId, homeId: null, agent: null }
}

/** What `widgetId` on `page` shows. Template controls act for the first
 *  Agents widget; a Conversation pinned with `agent` (and `home`) shows
 *  that one agent, else it follows its `agents` prop or the first Agents
 *  widget; anything else is a loose view. */
export function zenViewFor(page: ZenResolvedPage, gardenId: string, widgetId: string): ZenView | null {
  const w = page.widgets.find((x) => x.id === widgetId)
  if (!w) {
    if (widgetId !== ZEN_TEMPLATE_CONTROLS_ID) return null
    const first = firstAgentsWidget(page)
    return first ? agentsView(gardenId, first) : null
  }
  if (w.kind === 'agents') return agentsView(gardenId, w)
  if (w.kind === 'conversation') {
    if (propString(w.props, 'agent')) return homeView(gardenId, w)
    const named = propString(w.props, 'agents')
    const follow =
      (named ? page.widgets.find((x) => x.id === named && x.kind === 'agents') : undefined) ?? firstAgentsWidget(page)
    return follow ? agentsView(gardenId, follow) : looseView(gardenId, w.id)
  }
  return looseView(gardenId, w.id)
}

function viewOf(ctx: ZenVerbCtx): ZenView {
  const v = zenViewFor(ctx.page(), ctx.gardenId(), ctx.widgetId)
  if (!v) throw new Error(`zen: widget ${ctx.widgetId} has no agents`)
  return v
}

/** The calling widget's view (null: no Agents widget to act for). */
export function zenViewForCtx(ctx: ZenVerbCtx): ZenView | null {
  return zenViewFor(ctx.page(), ctx.gardenId(), ctx.widgetId)
}

/** The view Add agent acts for: the calling widget's own Home, else (the
 *  template's controls, a widget that shows no Home) the page's first
 *  Agents widget. */
export function zenAddTargetForCtx(ctx: ZenVerbCtx): ZenView | null {
  const own = zenViewForCtx(ctx)
  if (own?.homeId) return own
  const first = firstAgentsWidget(ctx.page())
  return first ? agentsView(ctx.gardenId(), first) : null
}

/** The Home id the calling widget's view shows (null: a loose view). */
export function zenHomeIdForCtx(ctx: ZenVerbCtx): string | null {
  return zenViewForCtx(ctx)?.homeId ?? null
}

function matchesAgent(row: HomeRow, agent: string): boolean {
  const a = agent.toLocaleLowerCase()
  if (row.address === a) return true
  const p = parseHomeAddress(row.address)
  return (p !== null && p.handle === a) || row.label.toLocaleLowerCase() === a
}

/** A row by address: on any Home, else an agent on this computer that
 *  `agents.local` listed. */
function anyRow(address: string): HomeRow | null {
  for (const h of useHomesStore.getState().homes) {
    const r = h.rows.find((x) => x.address === address)
    if (r) return r
  }
  return looseRows.get(address) ?? null
}

/** The view's rows, in Home order (the Agents view: the Agents page's). */
function viewRows(view: ZenView): HomeRow[] {
  if (view.source === 'workspaces') {
    const rows = workspaceRows()
    return view.agent ? rows.filter((r) => matchesAgent(r, view.agent as string)) : rows
  }
  if (view.homeId === null) {
    const address = selection.get(view.key)
    const row = address ? anyRow(address) : null
    return row ? [row] : []
  }
  const home = useHomesStore.getState().homes.find((h) => h.id === view.homeId)
  if (!home) return []
  return view.agent ? home.rows.filter((r) => matchesAgent(r, view.agent as string)) : home.rows
}

// ── Rows ───────────────────────────────────────────────────────────────────

function rowFor(row: HomeRow, index: number, view: ZenView | null): ZenAgentRow {
  const host = useConnectHostStore.getState()
  const projects = useProjectsStore.getState().projects
  const parsed = parseHomeAddress(row.address)
  const hostKey = parsed?.host ?? ''
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
    selected: view !== null && selection.get(view.key) === row.address,
    openable: state === 'ok',
  }
}

/** `agents.list()`: the view's rows, in Home order. */
export function zenAgentRows(view: ZenView | null): ZenAgentRow[] {
  if (!view) return []
  return viewRows(view).map((r, i) => rowFor(r, i, view))
}

function syncViewHomes(): void {
  const ids = new Set<string>()
  for (const l of rowListeners) {
    const v = l.view()
    if (v?.homeId) ids.add(v.homeId)
  }
  const next = [...ids].sort()
  const prev = zenViewHomes.getState().homeIds
  if (prev.length !== next.length || prev.some((id, i) => id !== next[i])) zenViewHomes.setState({ homeIds: next })
}

function emitRows(): void {
  if (rowListeners.size === 0) return
  for (const l of [...rowListeners]) {
    if (!rowListeners.has(l)) continue
    const rows = zenAgentRows(l.view())
    const sig = JSON.stringify(rows)
    if (sig === l.sig) continue
    l.sig = sig
    l.cb(rows)
  }
  syncViewHomes()
}

/** Every row a live view shows (deduplicated by address). */
function liveRows(): ZenAgentRow[] {
  const out = new Map<string, ZenAgentRow>()
  for (const l of rowListeners) for (const r of zenAgentRows(l.view())) if (!out.has(r.address)) out.set(r.address, r)
  return [...out.values()]
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
  unsubs.push(useFocusGroupsStore.subscribe(on))
  unsubs.push(useActiveAgentsStore.subscribe(on))
  unsubs.push(usePresenceStore.subscribe(on))
  unsubs.push(hostPool.store.subscribe(on))
  unsubs.push(useZenGardenHomesStore.subscribe(on))
  unsubs.push(useZenGardensStore.subscribe(on))
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
      for (const address of new Set(selection.values())) {
        const conv = conversations.get(address)
        if (conv && conv.phase === 'ready' && !isConnectedHost(conv.hostKey)) homeRooms.focused(address)
      }
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
  const rows = liveRows().filter((r) => r.state === 'ok')
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

/** The row `address` as `view` shows it: one of its rows, else (the Ask my
 *  agent pick, an agent on another Home) a row of its own. */
function findRow(view: ZenView, address: string): { row: HomeRow; index: number } {
  const rows = viewRows(view)
  const index = rows.findIndex((r) => r.address === address)
  if (index >= 0) return { row: rows[index], index }
  if (view.homeId === null && view.source !== 'workspaces') {
    const loose = anyRow(address)
    if (loose) return { row: loose, index: 0 }
  }
  throw new Error(`zen: ${address} is not an agent this widget shows`)
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

/** `conversation.open(address, {where?})` (Z35, Z37, Z43) in `view`. */
export async function openZenConversation(
  view: ZenView,
  address: string,
  opts: { where?: 'zen' | 'agents' } = {},
): Promise<ZenAgentRow> {
  const { row, index } = findRow(view, address)
  if (opts.where === 'agents') {
    await openInAgents(row)
    return rowFor(row, index, view)
  }
  // The loose view lists only its pick: select first, so the row is its own.
  selection.set(view.key, address)
  const current = rowFor(row, index, view)
  const gen = ++generation
  const base = { address, hostKey: current.hostKey, scope: null, threadAddr: null, workspacePath: '', generation: gen }
  if (current.state !== 'ok' && current.state !== 'checking') {
    setConversation({ ...base, phase: 'unavailable', note: current.detail ?? current.stateLabel })
    return rowFor(row, index, view)
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
      return rowFor(row, index, view)
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
      return rowFor(row, index, view)
    }
    // In place, whatever "Open agents from other servers here" says.
    const entry = await homeRooms.open(row, current.hostKey)
    if (stale()) return rowFor(row, index, view)
    if (entry.phase !== 'open' || !entry.room) {
      const note =
        entry.phase === 'not-found'
          ? `${row.label} is not on ${where} any more.`
          : entry.phase === 'switched'
            ? `Update ${where} to message this agent here.`
            : `Couldn’t open ${row.label}${entry.error ? `: ${entry.error}` : '.'}`
      setConversation({ ...base, phase: 'failed', note })
      return rowFor(row, index, view)
    }
    scope = scopeForHost(current.hostKey)
    workspacePath = entry.room.cwd()
    projectId = entry.room.activeProjectId()
  }
  if (existing && existing.phase === 'ready' && existing.scope === scope && existing.workspacePath === workspacePath) {
    // Already resolved: keep the feed (same generation), just re-show.
    syncFeeds()
    emitRows()
    return rowFor(row, index, view)
  }
  // vs-live Z52: the Agents page's own resolver, not the row handle.
  const resolved = await resolvePinnedChatCopyableAddress(scope, workspacePath, projectId)
  if (stale()) return rowFor(row, index, view)
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
  return rowFor(row, index, view)
}

/** "Open in Agents" (Z43, answer 10, G32, G54): leave Zen for the agent's
 *  terminal, where its permission prompt is. Zen turns off FIRST in both
 *  branches, so the page underneath mounts live. A row on the window's
 *  server opens on the Agents page; a row on another server only has a
 *  room on Home, so Home shows that room, in place. The window's server
 *  never changes. */
async function openInAgents(row: HomeRow): Promise<void> {
  const hostKey = parseHomeAddress(row.address)?.host ?? ''
  exitZen()
  if (isConnectedHost(hostKey)) {
    const ws = findWorkspaceForRow(useProjectsStore.getState().projects, row)
    if (ws) useProjectsStore.getState().setActiveProject(ws.id)
    homeRooms.showPrimary()
    usePageViewStore.getState().setPage('agents')
    return
  }
  usePageViewStore.getState().setPage('home')
  if (!hostPool.entry(hostKey)?.boot?.version) await hostPool.check(hostKey)
  if (homeRoomVerdict(hostPool.entry(hostKey)?.boot) === 'switch') return
  await homeRooms.open(row, hostKey)
}

function closeZenConversation(view: ZenView): void {
  selection.delete(view.key)
  emitRows()
}

/** ⌘1–9 / ⌘0 in Zen (G52): row `index` of the page's first Agents widget;
 *  nothing when the Garden has none. */
export function selectZenRowOnPage(page: ZenResolvedPage, gardenId: string, index: number): void {
  const first = firstAgentsWidget(page)
  if (!first) return
  const view = agentsView(gardenId, first)
  const row = viewRows(view)[index]
  if (!row) return
  void openZenConversation(view, row.address).catch((err: unknown) => console.warn('[zen] open failed:', err))
}

// ── Agents on this computer (G28, G61) ────────────────────────────────────

interface LocalWorkspace {
  id: string
  name: string
  handle?: string | null
}

function parseLocalWorkspaces(raw: unknown): LocalWorkspace[] {
  if (!Array.isArray(raw)) throw new Error('projects/list: not a list')
  const out: LocalWorkspace[] = []
  for (const w of raw) {
    if (!isObj(w) || typeof w.id !== 'string' || typeof w.name !== 'string') continue
    out.push({ id: w.id, name: w.name, handle: typeof w.handle === 'string' ? w.handle : null })
  }
  return out
}

/** This computer's workspaces: the window's own list when it is on this
 *  computer, else one `projects/list` on the local scope. */
async function localWorkspaces(): Promise<LocalWorkspace[]> {
  if (useConnectHostStore.getState().activeHost === 'local') return useProjectsStore.getState().projects
  return parseLocalWorkspaces(await daemonCliGet<unknown>(scopeForHost(LOCAL_HOME_HOST), 'projects/list'))
}

/** `agents.local()`: agents on this computer only — Home rows on this
 *  computer across every Home, plus this computer's workspaces —
 *  deduplicated by address. Only an agent on this computer can edit this
 *  computer's `~/.k2/zen`. */
export async function zenLocalAgents(view: ZenView | null): Promise<ZenAgentRow[]> {
  const rows = new Map<string, HomeRow>()
  for (const h of useHomesStore.getState().homes) {
    for (const r of h.rows) if (parseHomeAddress(r.address)?.host === LOCAL_HOME_HOST && !rows.has(r.address)) rows.set(r.address, r)
  }
  try {
    for (const w of await localWorkspaces()) {
      const handle = workspaceHandle(w)
      if (!handle) continue
      const address = homeAddress(handle, LOCAL_HOME_HOST)
      if (rows.has(address)) continue
      const row: HomeRow = { address, workspaceId: w.id, label: w.name }
      rows.set(address, row)
      if (!anyRow(address)) looseRows.set(address, row)
    }
  } catch (err) {
    console.warn('[zen] listing this computer’s agents failed:', err)
  }
  return [...rows.values()].map((r, i) => rowFor(r, i, view))
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

function rowsSubscribe(ctx: ZenVerbCtx, cb: (rows: ZenAgentRow[]) => void): () => void {
  const view = (): ZenView | null => zenViewFor(ctx.page(), ctx.gardenId(), ctx.widgetId)
  const rows = zenAgentRows(view())
  const listener: RowListener = { view, cb, sig: JSON.stringify(rows) }
  rowListeners.add(listener)
  startLive()
  syncViewHomes()
  cb(rows)
  return () => {
    rowListeners.delete(listener)
    syncViewHomes()
    if (rowListeners.size === 0) stopLive?.()
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

/** Register the data verbs. Returns the uninstall. */
export function installZenDataVerbs(): () => void {
  const offs = [
    registerZenVerb('agents.list', (ctx) => zenAgentRows(zenViewFor(ctx.page(), ctx.gardenId(), ctx.widgetId))),
    registerZenVerb('agents.subscribe', (ctx, cb) => rowsSubscribe(ctx, fn<ZenAgentRow[]>(cb, 'agents.subscribe callback'))),
    // G27: the Agents widget's own Home (view state; never `selectHome`).
    registerZenVerb('agents.home', (ctx) => zenHomeIdForCtx(ctx)),
    registerZenVerb('agents.setHome', (ctx, id) => {
      const homeId = str(id, 'a Home id')
      if (!useHomesStore.getState().homes.some((h) => h.id === homeId)) throw new Error(`zen: no Home ${homeId}`)
      const view = viewOf(ctx)
      if (view.homeId === null) throw new Error(`zen: widget ${ctx.widgetId} shows no Home`)
      useZenGardenHomesStore.getState().setPick(view.key, homeId)
      // A conversation from the old Home closes with it.
      const open = selection.get(view.key)
      if (open && !viewRows({ ...view, homeId }).some((r) => r.address === open)) selection.delete(view.key)
      emitRows()
    }),
    registerZenVerb('agents.local', (ctx) => zenLocalAgents(zenViewFor(ctx.page(), ctx.gardenId(), ctx.widgetId))),
    // The Agents view's focus-group dropdown: the app's own focus groups.
    registerZenVerb('focusGroups.get', () => zenFocusGroups()),
    registerZenVerb('focusGroups.set', (ctx, id) => {
      const groupId = str(id, 'a focus group id')
      const fg = useFocusGroupsStore.getState()
      if (!fg.focusGroupsEnabled) throw new Error('zen: focus groups are off')
      if (!fg.focusGroups.some((g) => g.id === groupId)) throw new Error(`zen: no focus group ${groupId}`)
      const view = viewOf(ctx)
      if (view.source !== 'workspaces') throw new Error(`zen: widget ${ctx.widgetId} shows no workspaces`)
      // Never switch workspaces: the list changes, nothing else.
      fg.setActiveFocusGroup(groupId, { autoActivate: false })
      // A conversation from the old group closes with it.
      const open = selection.get(view.key)
      if (open && !viewRows(view).some((r) => r.address === open)) selection.delete(view.key)
      emitRows()
    }),
    registerZenVerb('focusGroups.subscribe', (_ctx, cb) => {
      const f = fn<ZenFocusGroups>(cb, 'focusGroups.subscribe callback')
      let last = JSON.stringify(zenFocusGroups())
      return useFocusGroupsStore.subscribe(() => {
        const next = zenFocusGroups()
        const sig = JSON.stringify(next)
        if (sig === last) return
        last = sig
        f(next)
      })
    }),
    registerZenVerb('presence.get', (ctx, address) => {
      const a = str(address, 'address')
      return zenAgentRows(zenViewFor(ctx.page(), ctx.gardenId(), ctx.widgetId)).find((r) => r.address === a)?.people ?? []
    }),
    registerZenVerb('presence.subscribe', (ctx, address, cb) => {
      const a = str(address, 'address')
      const f = fn<ZenPerson[]>(cb, 'presence.subscribe callback')
      let last = ''
      return rowsSubscribe(ctx, (rows) => {
        const people = rows.find((r) => r.address === a)?.people ?? []
        const sig = JSON.stringify(people)
        if (sig === last) return
        last = sig
        f(people)
      })
    }),
    registerZenVerb('conversation.open', (ctx, address, opts) =>
      openZenConversation(
        viewOf(ctx),
        str(address, 'address'),
        isObj(opts) && (opts.where === 'agents' || opts.where === 'zen') ? { where: opts.where } : {},
      ),
    ),
    registerZenVerb('conversation.close', (ctx) => closeZenConversation(viewOf(ctx))),
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
    // G28, G60: set a conversation's message box; never sends.
    registerZenVerb('compose.draft', (_ctx, address, text) => {
      if (typeof text !== 'string') throw new Error('zen bridge: compose.draft text must be a string')
      draftZenCompose(str(address, 'address'), text)
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
  looseRows.clear()
  rowListeners.clear()
  threadListeners.clear()
  previewInFlight.clear()
  zenThreadFeeds.setState({ feeds: [] })
  zenViewHomes.setState({ homeIds: [] })
}

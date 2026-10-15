// Local Grok Bots (0.45.7) — thin client over `/cli/grokbots/*`.
//
// Grok Bot agents run on a server's computer for the daemon's own user.
// The daemon detects them, lists them, reads their chat and (only when the
// owner turns Send on) sends to them as the person. They are NOT K2
// workspaces: no PTY, no session, no tabs. Every shape here is parsed
// defensively — the daemon is the truth, and older servers have no route
// (`scope.serverSupports('grokbots-v1')` says whether to ask).
//
// On Home a Grok Bot row is an ordinary `HomeRow` whose handle is
// `grokbot-<agent id>` (see `grokBotHomeHandle`): no new row field, so the
// homes store, its storage sync and the Garden projections stay unchanged.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { parseHomeAddress } from '@/lib/home-address'
import type { RowStatus } from '@/lib/home-status'

export const GROK_BOT_LABEL = 'Grok Bot'
export const GROK_BOTS_FEATURE = 'grokbots-v1' as const
export const GROK_BOTS_SETTINGS_PATH = 'Settings → 3rd Party Bots → Grok Bot'

export interface GrokBotAgent {
  id: string
  name: string
  title: string | null
  /** `data:image/…` or null. */
  avatar: string | null
  color: string | null
  /** `gateway` (live) or `disk` (Grok Bot not running). */
  source: 'gateway' | 'disk'
  working: boolean
  needsYou: boolean
  unreadCount: number
  /** The bot's live conversation runs outside this computer (another box
   *  or Grok's cloud): the chat K2 reads here is old. Sends still reach the
   *  live conversation; its new lines show in the thread's live tail. */
  stale: boolean
  /** The latest message's preview (current even when stale: the daemon
   *  uses the live tail's newest line when that is newer). */
  lastMessagePreview: string | null
  /** Latest activity anywhere (ms). */
  lastActivityAt: number | null
  /** How far this computer's copy of the chat goes (ms). */
  localActivityAt: number | null
}

/** The note under a stale bot's compose box. */
export const GROK_STALE_COMPOSE_NOTE =
  "Replies come from Grok Bot's live conversation; older history on this computer may be out of date."

/** Shown before a live line when the bot may have written more in between
 *  (only the latest line at each check is visible to K2). */
export const GROK_LIVE_GAP_TEXT = '… earlier messages not shown — open Grok Bot'

/** The divider between a stale bot's old local history and its live tail. */
export function grokLiveDividerText(sinceMs: number): string {
  const when = new Date(sinceMs).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  })
  return `Live since ${when} — older history above may be out of date`
}

export interface GrokBotStatus {
  reachable: boolean
  busy: boolean
  activeAgentId: string | null
}

export interface GrokBotList {
  /** Settings → Local Grok Bots → Show. */
  enabled: boolean
  /** Show and Send are both on. */
  sendEnabled: boolean
  installed: boolean
  status: GrokBotStatus
  agents: GrokBotAgent[]
}

export type GrokCard =
  | {
      type: 'choice'
      prompt: string
      helpText: string | null
      options: { label: string; description: string | null; chosen: boolean }[]
      state: 'open' | 'answered' | 'skipped' | 'dismissed'
      customAnswer: string | null
    }
  | { type: 'connector'; connector: string; variant: string | null }
  | { type: 'secret-request'; label: string; description: string | null; platform: string | null; provided: boolean }
  | { type: 'template-share'; name: string; body: string | null; shareUrl: string | null }

export interface GrokMessage {
  seq: number
  author: 'human' | 'agent' | 'peer'
  text: string
  atMs: number | null
  toAgent: { id: string; name: string } | null
  fromAgent: { id: string; name: string } | null
  viaK2: boolean
  /** Set when K2 can't show the line (names what it is). */
  openInApp: string | null
  card: GrokCard | null
}

/** One line of a stale bot's live tail: what Grok Bot's roster summary
 *  showed as the bot's latest line at one of the server's checks, or a
 *  message sent from K2. */
export interface GrokLiveEntry {
  id: string
  author: 'human' | 'agent' | 'peer'
  text: string
  atMs: number | null
  viaK2: boolean
  /** Lines may be missing just before this one. */
  gapBefore: boolean
  /** The text is Grok Bot's one-line preview (may be cut short). */
  previewOnly: boolean
}

export interface GrokLiveTail {
  /** When the server started following the bot's live conversation. */
  sinceMs: number
  entries: GrokLiveEntry[]
}

export interface GrokThreadPage {
  supported: boolean
  reason: string | null
  messages: GrokMessage[]
  nextSince: number
  hasMoreBefore: boolean
  /** The bot's live conversation runs elsewhere (then `liveTail` is set). */
  stale: boolean
  liveTail: GrokLiveTail | null
}

export interface GrokSettings {
  show: boolean
  send: boolean
  canEdit: boolean
}

export interface GrokDetection {
  installed: boolean
  dataPath: string
  agents: GrokBotAgent[]
}

const str = (v: unknown): string | null => (typeof v === 'string' && v !== '' ? v : null)
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' ? (v as Record<string, unknown>) : {})
const ref = (v: unknown): { id: string; name: string } | null => {
  const o = obj(v)
  const id = str(o.id)
  return id ? { id, name: str(o.name) ?? id } : null
}

export function parseGrokAgent(raw: unknown): GrokBotAgent | null {
  const o = obj(raw)
  const id = str(o.id)
  if (!id) return null
  const avatar = str(o.avatar)
  return {
    id,
    name: str(o.name) ?? id,
    title: str(o.title),
    avatar: avatar && avatar.startsWith('data:image/') ? avatar : null,
    color: str(o.color),
    source: o.source === 'gateway' ? 'gateway' : 'disk',
    working: o.working === true,
    needsYou: o.needsYou === true,
    unreadCount: typeof o.unreadCount === 'number' ? o.unreadCount : 0,
    stale: o.stale === true,
    lastMessagePreview: str(o.lastMessagePreview),
    lastActivityAt: typeof o.lastActivityAt === 'number' && o.lastActivityAt > 0 ? o.lastActivityAt : null,
    localActivityAt: typeof o.localActivityAt === 'number' && o.localActivityAt > 0 ? o.localActivityAt : null,
  }
}

export function parseGrokList(raw: unknown): GrokBotList {
  const o = obj(raw)
  const st = obj(o.status)
  return {
    enabled: o.enabled === true,
    sendEnabled: o.sendEnabled === true,
    installed: o.installed === true,
    status: {
      reachable: st.reachable === true,
      busy: st.busy === true,
      activeAgentId: str(st.activeAgentId),
    },
    agents: Array.isArray(o.agents) ? o.agents.map(parseGrokAgent).filter((a): a is GrokBotAgent => a !== null) : [],
  }
}

function parseCard(raw: unknown): GrokCard | null {
  const o = obj(raw)
  switch (o.type) {
    case 'choice': {
      const state = o.state
      return {
        type: 'choice',
        prompt: str(o.prompt) ?? '',
        helpText: str(o.helpText),
        options: Array.isArray(o.options)
          ? o.options.map((x) => {
              const p = obj(x)
              return { label: str(p.label) ?? '', description: str(p.description), chosen: p.chosen === true }
            })
          : [],
        state: state === 'answered' || state === 'skipped' || state === 'dismissed' ? state : 'open',
        customAnswer: str(o.customAnswer),
      }
    }
    case 'connector':
      return { type: 'connector', connector: str(o.connector) ?? '', variant: str(o.variant) }
    case 'secret-request':
      // Never a value: the daemon only says whether it was provided.
      return {
        type: 'secret-request',
        label: str(o.label) ?? '',
        description: str(o.description),
        platform: str(o.platform),
        provided: o.provided === true,
      }
    case 'template-share': {
      const url = str(o.shareUrl)
      return {
        type: 'template-share',
        name: str(o.name) ?? '',
        body: str(o.body),
        shareUrl: url && url.startsWith('https://') ? url : null,
      }
    }
    default:
      return null
  }
}

export function parseGrokMessage(raw: unknown): GrokMessage | null {
  const o = obj(raw)
  if (typeof o.seq !== 'number') return null
  const author = o.author === 'human' || o.author === 'peer' ? o.author : 'agent'
  return {
    seq: o.seq,
    author,
    text: typeof o.text === 'string' ? o.text : '',
    atMs: typeof o.atMs === 'number' ? o.atMs : null,
    toAgent: ref(o.toAgent),
    fromAgent: ref(o.fromAgent),
    viaK2: o.viaK2 === true,
    openInApp: str(o.openInApp),
    card: parseCard(o.card),
  }
}

export function parseGrokLiveEntry(raw: unknown): GrokLiveEntry | null {
  const o = obj(raw)
  const id = str(o.id)
  if (!id || typeof o.text !== 'string') return null
  return {
    id,
    author: o.author === 'human' || o.author === 'peer' ? o.author : 'agent',
    text: o.text,
    atMs: typeof o.atMs === 'number' && o.atMs > 0 ? o.atMs : null,
    viaK2: o.viaK2 === true,
    gapBefore: o.gapBefore === true,
    previewOnly: o.previewOnly === true,
  }
}

export function parseGrokLiveTail(raw: unknown): GrokLiveTail | null {
  if (!raw || typeof raw !== 'object') return null
  const o = obj(raw)
  if (typeof o.sinceMs !== 'number') return null
  return {
    sinceMs: o.sinceMs,
    entries: Array.isArray(o.entries)
      ? o.entries.map(parseGrokLiveEntry).filter((e): e is GrokLiveEntry => e !== null)
      : [],
  }
}

export function parseGrokThread(raw: unknown): GrokThreadPage {
  const o = obj(raw)
  const liveTail = parseGrokLiveTail(o.liveTail)
  return {
    supported: o.supported !== false,
    reason: str(o.reason),
    messages: Array.isArray(o.messages)
      ? o.messages.map(parseGrokMessage).filter((m): m is GrokMessage => m !== null)
      : [],
    nextSince: typeof o.nextSince === 'number' ? o.nextSince : 0,
    hasMoreBefore: o.hasMoreBefore === true,
    stale: o.stale === true && liveTail !== null,
    liveTail,
  }
}

/** A live-tail line in the chat's message shape (its `seq` is not a store
 *  cursor and is never merged into the local history). */
export function grokLiveEntryAsMessage(e: GrokLiveEntry): GrokMessage {
  return {
    seq: -1,
    author: e.author,
    text: e.text,
    atMs: e.atMs,
    toAgent: null,
    fromAgent: null,
    viaK2: e.viaK2,
    openInApp: null,
    card: null,
  }
}

export function parseGrokSettings(raw: unknown): GrokSettings {
  const o = obj(raw)
  return { show: o.show === true, send: o.send === true, canEdit: o.canEdit === true }
}

/** The daemon stamps every K2 send `[from <name>] <text>` (name: no `[`,
 *  `]` or line breaks, at most 64 chars) so the bot knows who wrote. */
const K2_FROM_ENVELOPE = /^\[from ([^\[\]\r\n]{1,64})\] /

/** Who sent a line from K2, and the text without K2's `[from …]` stamp.
 *  Only a human line K2 sent (`viaK2`) is read this way; any other line is
 *  shown exactly as stored. */
export function grokK2Sender(m: GrokMessage): { from: string | null; text: string } {
  if (!m.viaK2 || m.author !== 'human') return { from: null, text: m.text }
  const hit = K2_FROM_ENVELOPE.exec(m.text)
  if (!hit) return { from: null, text: m.text }
  return { from: hit[1].trim() || null, text: m.text.slice(hit[0].length) }
}

/** Merge a fresh page into what's on screen, by seq (a later read of the
 *  same seq replaces it: Grok Bot may update a line in place). */
export function mergeGrokMessages(prev: GrokMessage[], next: GrokMessage[]): GrokMessage[] {
  if (next.length === 0) return prev
  const bySeq = new Map(prev.map((m) => [m.seq, m]))
  for (const m of next) bySeq.set(m.seq, m)
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq)
}

// ── routes ──────────────────────────────────────────────────────────────

export async function fetchGrokBots(scope: ServerScope): Promise<GrokBotList> {
  return parseGrokList(await daemonCliGet<unknown>(scope, 'grokbots/list'))
}

export async function fetchGrokThread(
  scope: ServerScope,
  agent: string,
  opts: { since?: number; limit?: number } = {},
): Promise<GrokThreadPage> {
  return parseGrokThread(
    await daemonCliGet<unknown>(scope, 'grokbots/thread', {
      agent,
      since: opts.since,
      limit: opts.limit,
    }),
  )
}

export async function sendToGrokBot(scope: ServerScope, agent: string, text: string): Promise<void> {
  await daemonCliPost<unknown>(scope, 'grokbots/send', { agent, text })
}

export async function fetchGrokSettings(scope: ServerScope): Promise<GrokSettings> {
  return parseGrokSettings(await daemonCliGet<unknown>(scope, 'grokbots/settings'))
}

export async function saveGrokSettings(
  scope: ServerScope,
  change: { show?: boolean; send?: boolean },
): Promise<GrokSettings> {
  return parseGrokSettings(await daemonCliPost<unknown>(scope, 'grokbots/settings', change))
}

export async function detectGrokBots(scope: ServerScope): Promise<GrokDetection> {
  const o = obj(await daemonCliGet<unknown>(scope, 'grokbots/detect'))
  return {
    installed: o.installed === true,
    dataPath: str(o.dataPath) ?? '',
    agents: Array.isArray(o.agents)
      ? o.agents.map(parseGrokAgent).filter((a): a is GrokBotAgent => a !== null)
      : [],
  }
}

// ── Home addressing ─────────────────────────────────────────────────────

const HANDLE_PREFIX = 'grokbot-'

/** A Grok Bot's Home handle: `grokbot-<agent id>` (lowercase). */
export function grokBotHomeHandle(agentId: string): string {
  return `${HANDLE_PREFIX}${agentId.toLowerCase()}`
}

/** The agent id when `address` (`handle::host`) is a Grok Bot row. */
export function grokBotIdFromAddress(address: string): string | null {
  const parsed = parseHomeAddress(address)
  if (!parsed || !parsed.handle.startsWith(HANDLE_PREFIX)) return null
  const id = parsed.handle.slice(HANDLE_PREFIX.length)
  return /^[a-z0-9_-]{1,128}$/.test(id) ? id : null
}

export function isGrokBotAddress(address: string): boolean {
  return grokBotIdFromAddress(address) !== null
}

// ── status words ────────────────────────────────────────────────────────

export type GrokAgentState = 'working' | 'needs-you' | 'idle' | 'not-running'

/** What a Grok Bot row says. Grok Bot runs one turn at a time for the
 *  whole computer, so the box-wide busy flag also marks its active agent. */
export function grokAgentState(agent: GrokBotAgent, status: GrokBotStatus): GrokAgentState {
  if (agent.needsYou) return 'needs-you'
  if (agent.working || (status.busy && status.activeAgentId?.toLowerCase() === agent.id.toLowerCase())) {
    return 'working'
  }
  if (agent.source === 'disk' || !status.reachable) return 'not-running'
  return 'idle'
}

export const GROK_STATE_LABEL: Record<GrokAgentState, string> = {
  working: 'Working',
  'needs-you': 'Needs you',
  idle: 'Idle',
  'not-running': 'Grok Bot not running',
}

/** What one server said about its Grok Bots (the store's per-host slot). */
export interface GrokHostSnapshot {
  list: GrokBotList | null
  error: string | null
  loading: boolean
}

/** A Grok Bot Home row's status in Home's row vocabulary (rail, ⌘1–9,
 *  Garden rows). `base` is the server's own status for a row on another
 *  server (offline / sign in / no access win). */
export function grokRowStatus(
  snap: GrokHostSnapshot | undefined,
  agentId: string,
  base: RowStatus | null,
): RowStatus {
  if (base && ['offline', 'sign-in', 'signing-in', 'no-access', 'starting', 'checking'].includes(base.kind)) {
    return base
  }
  if (!snap || (!snap.list && !snap.error)) return { kind: 'checking', label: 'Checking…', people: [] }
  if (snap.error || !snap.list) {
    return { kind: 'not-found', label: 'Unavailable', people: [], detail: "Can't reach Grok Bots on that server" }
  }
  if (!snap.list.enabled) {
    return { kind: 'not-found', label: 'Off', people: [], detail: 'Local Grok Bots are off on that server' }
  }
  const id = agentId.toLowerCase()
  const agent = snap.list.agents.find((a) => a.id.toLowerCase() === id)
  if (!agent) return { kind: 'not-found', label: 'Not found', people: [] }
  const state = grokAgentState(agent, snap.list.status)
  if (state === 'working') return { kind: 'working', label: 'Working', people: [] }
  if (state === 'needs-you') return { kind: 'permission', label: 'Needs you', people: [] }
  if (state === 'not-running') return { kind: 'idle', label: 'Not running', people: [] }
  return { kind: 'idle', label: 'Idle', people: [] }
}

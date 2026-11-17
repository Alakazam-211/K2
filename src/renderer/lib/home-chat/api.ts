// People and channels on Home: the client side of `/cli/home-chat/*`
// (prd-home-people-channels-v1 §14.4). Every call goes to the server that
// holds the chat, with this person's own login there (`ServerScope`). The
// daemon talks to Sew as the person; this file never reaches Sew itself and
// never touches the Thread store (agent Threads stay on redb, §2).
//
// Sew's shapes (ids are decimal strings, times RFC 3339) are read into small
// client types here, so the views never parse Sew JSON.

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { parseCallSummary, type HomeChatCallSummary } from './calls'
import { parseRecordingSummary, transcriptRootOf, type HomeChatRecordingSummary } from './recordings'

export const HOME_CHAT_FEATURE = 'home-chat-v1'

// ── Types ──────────────────────────────────────────────────────────────

/** Why chat isn't usable on a server (`/cli/home-chat/status` `reason`). */
export type HomeChatOffReason = 'off' | 'mac' | 'sew_off' | 'space_pending'

export interface HomeChatStatus {
  on: boolean
  reason: HomeChatOffReason | string | null
  /** Sew's Home flags (`read_state_unread_count`, `http_typing`, …). */
  flags: Record<string, boolean>
}

/** One person the caller may DM on a server (§6.1). */
export interface HomeChatPerson {
  ext: string
  username: string
  displayName: string
  picture: string | null
  /** Shown to an Owner or Admin caller only. */
  role: string | null
  /** False while their chat is still being set up ("Setting up…"). */
  ready: boolean
  /** Their Sew member id (authors are named by it); null while pending. */
  memberId: string | null
}

export interface HomeChatPeople {
  me: { ext: string; memberId: string | null }
  people: HomeChatPerson[]
}

export type HomeChatBadge = { kind: 'none' } | { kind: 'dot' } | { kind: 'count'; n: number }

/** One conversation's unread, from the daemon's follower (§15.1). */
export interface HomeChatSummaryRow {
  conv: string
  kind: 'dm' | 'channel'
  unread: boolean
  badge: HomeChatBadge
  lastAt: string | null
  lastMessageId: string | null
  /** A DM's other person (their external id), when K2 kept the DM. */
  with: string | null
  /** A channel is invite-only (true) or open (false); null for a DM, or
   *  while the server hasn't said. Data for the row's glyph. */
  inviteOnly: boolean | null
}

export interface HomeChatSummary {
  /** `live`, `connecting` or `unavailable` (Sew down: badges grey). */
  chat: string
  convs: HomeChatSummaryRow[]
}

/** One emoji's chip under a bubble. */
export interface HomeChatReaction {
  emoji: string
  count: number
  /** The viewer reacted with it. */
  me: boolean
}

/** One message as the chat view draws it. */
export interface HomeChatMessage {
  id: string
  conv: string
  authorId: string
  body: string
  /** Epoch seconds. */
  createdAt: number
  seq: number
  reactions: HomeChatReaction[]
  /** The message this one replies to (Sew's inline `reference`, kind reply). */
  replyTo: string | null
  deleted: boolean
  nonce: string | null
  /** A DM call's record (Sew's CallSummary): drawn as a line, not a bubble. */
  call?: HomeChatCallSummary | null
  /** A recording's card (Sew's RecordingSummary): drawn as a line with Show
   *  transcript (prd-home-call-recording-v1 §2). */
  recording?: HomeChatRecordingSummary | null
  /** A transcript line: the card (or recording) it belongs to. Never on the
   *  DM's timeline; Show transcript reads it. */
  transcriptOf?: string | null
}

export interface HomeChatPage {
  messages: HomeChatMessage[]
  hasMore: boolean
}

/** A browsable K2 text channel on a server (§7.2). */
export interface HomeChatChannel {
  conv: string
  name: string
  open: boolean
  /** Invite-only (the inverse of `open`, as the server says it). */
  inviteOnly: boolean
  archived: boolean
  /** Made from Home (K2's ledger knows it). */
  home: boolean
  allowAgents: boolean
}

export interface HomeChatMembers {
  conv: string | null
  slug: string
  state: string
  name: string
  open: boolean
  creator: string
  allowAgents: boolean
  people: Array<{ ext: string; state: string }>
  /** Agents with access (S8: grants). Each `{name, …}` as the daemon sends. */
  agents: Array<Record<string, unknown>>
}

// ── Parsing ────────────────────────────────────────────────────────────

function idOf(v: unknown): string | null {
  if (typeof v === 'string' && v) return v
  if (typeof v === 'number' && Number.isFinite(v)) return String(v)
  return null
}

function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}

/** Compare two Sew ids (decimal strings, any length) numerically. */
export function compareSewIds(a: string, b: string): number {
  if (a.length !== b.length) return a.length - b.length
  return a < b ? -1 : a > b ? 1 : 0
}

function secondsOf(v: unknown): number {
  if (typeof v === 'number') return v > 1e12 ? Math.floor(v / 1000) : v
  if (typeof v === 'string') {
    const ms = Date.parse(v)
    if (Number.isFinite(ms)) return Math.floor(ms / 1000)
  }
  return 0
}

function reactionOf(v: unknown): HomeChatReaction | null {
  if (!v || typeof v !== 'object') return null
  const r = v as { emoji?: { kind?: unknown; value?: unknown }; count?: unknown; me?: unknown }
  // v1 is Unicode only (custom emoji are later, §10).
  if (r.emoji?.kind !== 'unicode' || typeof r.emoji.value !== 'string') return null
  const count = typeof r.count === 'number' ? r.count : 0
  if (count <= 0) return null
  return { emoji: r.emoji.value, count, me: r.me === true }
}

/** A Sew message (or a K2 frame's `message`) as the view's type. */
export function parseHomeChatMessage(v: unknown): HomeChatMessage | null {
  if (!v || typeof v !== 'object') return null
  const m = v as Record<string, unknown>
  const id = idOf(m.id)
  const conv = idOf(m.channel_id)
  if (!id || !conv) return null
  const ref = m.reference as { kind?: unknown; message_id?: unknown } | null | undefined
  return {
    id,
    conv,
    authorId: idOf(m.author_id) ?? '',
    body: str(m.body),
    createdAt: secondsOf(m.created_at),
    seq: typeof m.seq === 'number' ? m.seq : 0,
    reactions: Array.isArray(m.reactions) ? m.reactions.map(reactionOf).filter((r): r is HomeChatReaction => r !== null) : [],
    replyTo: ref && ref.kind === 'reply' ? idOf(ref.message_id) : null,
    deleted: m.deleted_at !== undefined && m.deleted_at !== null,
    nonce: typeof m.nonce === 'string' ? m.nonce : null,
    call: m.call ? parseCallSummary(m.call) : null,
    recording: m.recording ? parseRecordingSummary(m.recording) : null,
    transcriptOf: transcriptRootOf(m),
  }
}

/** A page answer (`{page: {items, has_more}}`): oldest first. */
export function parseHomeChatPage(v: unknown): HomeChatPage {
  const page = (v as { page?: unknown } | null)?.page as { items?: unknown; has_more?: unknown } | unknown[] | undefined
  const items = Array.isArray(page) ? page : Array.isArray((page as { items?: unknown })?.items) ? ((page as { items: unknown[] }).items) : []
  // A recording's transcript lines never join the timeline (HR-10).
  const messages = items.map(parseHomeChatMessage).filter((m): m is HomeChatMessage => m !== null && !m.transcriptOf)
  messages.sort((a, b) => compareSewIds(a.id, b.id))
  return { messages, hasMore: !Array.isArray(page) && (page as { has_more?: unknown })?.has_more === true }
}

function badgeOf(v: unknown): HomeChatBadge {
  const b = v as { kind?: unknown; n?: unknown } | null
  if (b?.kind === 'count' && typeof b.n === 'number' && b.n > 0) return { kind: 'count', n: b.n }
  if (b?.kind === 'dot') return { kind: 'dot' }
  return { kind: 'none' }
}

export function parseHomeChatSummary(v: unknown): HomeChatSummary {
  const o = (v ?? {}) as { chat?: unknown; convs?: unknown }
  const convs: HomeChatSummaryRow[] = []
  for (const r of Array.isArray(o.convs) ? o.convs : []) {
    const row = r as Record<string, unknown>
    const conv = idOf(row.conv)
    if (!conv) continue
    convs.push({
      conv,
      kind: row.kind === 'dm' ? 'dm' : 'channel',
      unread: row.unread === true,
      badge: badgeOf(row.badge),
      lastAt: typeof row.lastAt === 'string' ? row.lastAt : null,
      lastMessageId: idOf(row.lastMessageId),
      with: typeof row.with === 'string' ? row.with : null,
      inviteOnly: typeof row.inviteOnly === 'boolean' ? row.inviteOnly : null,
    })
  }
  return { chat: typeof o.chat === 'string' ? o.chat : 'unavailable', convs }
}

export function parseHomeChatPeople(v: unknown): HomeChatPeople {
  const o = (v ?? {}) as { me?: { ext?: unknown; memberId?: unknown }; people?: unknown }
  const people: HomeChatPerson[] = []
  for (const p of Array.isArray(o.people) ? o.people : []) {
    const r = p as Record<string, unknown>
    if (typeof r.ext !== 'string') continue
    people.push({
      ext: r.ext,
      username: str(r.username),
      displayName: str(r.displayName) || str(r.username),
      picture: typeof r.picture === 'string' && r.picture.startsWith('data:image/') ? r.picture : null,
      role: typeof r.role === 'string' ? r.role : null,
      ready: r.ready === true,
      memberId: idOf(r.memberId),
    })
  }
  return { me: { ext: str(o.me?.ext), memberId: idOf(o.me?.memberId) }, people }
}

// ── Calls ──────────────────────────────────────────────────────────────

// Whole route literals, never built from a helper: route_policy's client
// check reads the literal each daemonCliGet names, and `'home-chat/status'` read
// as the POST-only /cli/status.
export async function fetchHomeChatStatus(scope: ServerScope): Promise<HomeChatStatus> {
  const v = await daemonCliGet<{ on?: unknown; reason?: unknown; flags?: unknown }>(scope, 'home-chat/status')
  const flags: Record<string, boolean> = {}
  if (v.flags && typeof v.flags === 'object') {
    for (const [k, x] of Object.entries(v.flags as Record<string, unknown>)) flags[k] = x === true
  }
  return { on: v.on === true, reason: typeof v.reason === 'string' ? v.reason : null, flags }
}

export async function fetchHomeChatPeople(scope: ServerScope): Promise<HomeChatPeople> {
  return parseHomeChatPeople(await daemonCliGet(scope, 'home-chat/people'))
}

export async function fetchHomeChatSummary(scope: ServerScope, convs?: readonly string[]): Promise<HomeChatSummary> {
  return parseHomeChatSummary(await daemonCliGet(scope, 'home-chat/summary', convs && convs.length ? { convs: convs.join(',') } : undefined))
}

export async function fetchHomeChatChannels(scope: ServerScope, q?: string): Promise<HomeChatChannel[]> {
  const v = await daemonCliGet<{ channels?: unknown }>(scope, 'home-chat/channels', q ? { q } : undefined)
  const out: HomeChatChannel[] = []
  for (const c of Array.isArray(v.channels) ? v.channels : []) {
    const r = c as Record<string, unknown>
    const conv = idOf(r.conv)
    if (!conv) continue
    out.push({
      conv,
      name: str(r.name) || conv,
      open: r.open === true,
      inviteOnly: typeof r.inviteOnly === 'boolean' ? r.inviteOnly : r.open !== true,
      archived: r.archived === true,
      home: r.home === true,
      allowAgents: r.allowAgents === true,
    })
  }
  return out
}

/** Open (or make) the DM with `ext`: its conversation id. */
export async function openHomeChatDm(scope: ServerScope, ext: string): Promise<string> {
  const v = await daemonCliPost<{ conv?: unknown }>(scope, 'home-chat/dm', { ext })
  const conv = idOf(v.conv)
  if (!conv) throw new Error('the server answered no conversation for that DM')
  return conv
}

export async function fetchHomeChatMessages(
  scope: ServerScope,
  conv: string,
  opts: { before?: string; after?: string; limit?: number } = {},
): Promise<HomeChatPage> {
  return parseHomeChatPage(
    await daemonCliGet(scope, 'home-chat/messages', {
      conv,
      limit: opts.limit ?? 50,
      ...(opts.before ? { before: opts.before } : {}),
      ...(opts.after ? { after: opts.after } : {}),
    }),
  )
}

export async function sendHomeChatMessage(
  scope: ServerScope,
  body: { conv: string; text: string; nonce: string; replyTo?: string | null },
): Promise<HomeChatMessage | null> {
  const payload: Record<string, string> = { conv: body.conv, text: body.text, nonce: body.nonce }
  if (body.replyTo) payload.replyTo = body.replyTo
  const v = await daemonCliPost<{ message?: unknown }>(scope, 'home-chat/send', payload)
  return parseHomeChatMessage(v.message)
}

export async function reactHomeChat(
  scope: ServerScope,
  body: { conv: string; msg: string; emoji: string; on: boolean },
): Promise<HomeChatMessage | null> {
  const v = await daemonCliPost<{ message?: unknown }>(scope, 'home-chat/react', body)
  return parseHomeChatMessage(v.message)
}

/** Who reacted with `emoji` on `msg`: their member ids. */
export async function fetchHomeChatReactors(scope: ServerScope, msg: string, emoji: string): Promise<string[]> {
  const v = await daemonCliGet<{ reactors?: { items?: unknown } }>(scope, 'home-chat/reactors', { msg, emoji: `u:${emoji}` })
  const items = Array.isArray(v.reactors?.items) ? (v.reactors.items as unknown[]) : []
  return items.map((r) => idOf((r as { member_id?: unknown }).member_id)).filter((x): x is string => x !== null)
}

export async function ackHomeChat(scope: ServerScope, conv: string): Promise<void> {
  await daemonCliPost(scope, 'home-chat/ack', { conv })
}

export async function typingHomeChat(scope: ServerScope, conv: string): Promise<void> {
  await daemonCliPost(scope, 'home-chat/typing', { conv })
}

export async function viewHomeChat(scope: ServerScope, conv: string, open: boolean): Promise<void> {
  await daemonCliPost(scope, 'home-chat/view', { conv, open })
}

export async function createHomeChatChannel(
  scope: ServerScope,
  body: { name: string; open: boolean; members: string[] },
): Promise<{ slug: string; conv: string | null; state: string }> {
  const v = await daemonCliPost<{ slug?: unknown; conv?: unknown; state?: unknown }>(scope, 'home-chat/channels/create', body)
  return { slug: str(v.slug), conv: idOf(v.conv), state: str(v.state) }
}

export async function fetchHomeChatMembers(scope: ServerScope, by: { conv: string } | { slug: string }): Promise<HomeChatMembers> {
  const v = await daemonCliGet<Record<string, unknown>>(scope, 'home-chat/channels/members', by)
  return {
    conv: idOf(v.conv),
    slug: str(v.slug),
    state: str(v.state),
    name: str(v.name),
    open: v.open === true,
    creator: str(v.creator),
    allowAgents: v.allowAgents === true,
    people: Array.isArray(v.people)
      ? (v.people as Array<Record<string, unknown>>).filter((p) => typeof p.ext === 'string').map((p) => ({ ext: p.ext as string, state: str(p.state) }))
      : [],
    agents: Array.isArray(v.agents) ? (v.agents as Array<Record<string, unknown>>) : [],
  }
}

export async function addHomeChatPeople(
  scope: ServerScope,
  conv: string,
  ext: string[],
): Promise<Array<{ ext: string; outcome: string }>> {
  const v = await daemonCliPost<{ results?: unknown }>(scope, 'home-chat/channels/add', { conv, ext })
  return Array.isArray(v.results) ? (v.results as Array<{ ext: string; outcome: string }>) : []
}

export async function removeHomeChatPerson(scope: ServerScope, conv: string, ext: string): Promise<void> {
  await daemonCliPost(scope, 'home-chat/channels/remove', { conv, ext })
}

export async function leaveHomeChatChannel(scope: ServerScope, conv: string): Promise<void> {
  await daemonCliPost(scope, 'home-chat/channels/leave', { conv })
}

export async function renameHomeChatChannel(scope: ServerScope, conv: string, name: string): Promise<void> {
  await daemonCliPost(scope, 'home-chat/channels/rename', { conv, name })
}

export async function deleteHomeChatChannel(scope: ServerScope, conv: string): Promise<void> {
  await daemonCliPost(scope, 'home-chat/channels/delete', { conv })
}

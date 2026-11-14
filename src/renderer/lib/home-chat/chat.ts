// The Home chat view's pure rules (prd-home-people-channels-v1 §8–§11):
// merging pages and live messages, reactions (§10), reply previews and the
// jump (§11), who is typing (§9), names, and the view's in-memory copy.
//
// Storage rule (§2 non-goals): DMs and channels come from Sew through the
// daemon's `/cli/home-chat/*` only. The copy kept here is this window's
// memory (paint at once on reopening, then read the delta), never the
// Thread's store and never on disk.

import { compareSewIds, type HomeChatMessage, type HomeChatPeople, type HomeChatReaction } from './api'

/** The picker's quick row (§10). */
export const QUICK_REACTIONS = ['👍', '❤️', '😂', '🎉', '😮', '🙏'] as const

/** Pages the jump reads back before it says "Original not loaded" (§11). */
export const JUMP_MAX_PAGES = 20

/** A typing frame goes at most this often per conversation (§9.1). */
export const TYPING_INTERVAL_MS = 8_000

/** Messages kept per chat in this window's memory, and chats kept. */
export const VIEW_CACHE_MESSAGES = 100
export const VIEW_CACHE_CHATS = 40

/** Merge messages by id (a later copy wins), oldest first. */
export function mergeHomeChatMessages(prev: readonly HomeChatMessage[], incoming: readonly HomeChatMessage[]): HomeChatMessage[] {
  if (incoming.length === 0) return prev as HomeChatMessage[]
  const byId = new Map(prev.map((m) => [m.id, m]))
  for (const m of incoming) byId.set(m.id, m)
  return [...byId.values()].sort((a, b) => compareSewIds(a.id, b.id))
}

/** A live reaction frame on `messages`: the chip's new count from the frame;
 *  `me` changes only for the viewer's own reaction. A count of 0 drops it. */
export function applyReactionFrame(
  messages: readonly HomeChatMessage[],
  f: { msg: string; emoji: string; on: boolean; count: number; mine: boolean },
): HomeChatMessage[] {
  let changed = false
  const out = messages.map((m) => {
    if (m.id !== f.msg) return m
    changed = true
    return { ...m, reactions: withReaction(m.reactions, f.emoji, f.count, f.mine ? f.on : null) }
  })
  return changed ? out : (messages as HomeChatMessage[])
}

function withReaction(list: readonly HomeChatReaction[], emoji: string, count: number, me: boolean | null): HomeChatReaction[] {
  const at = list.findIndex((r) => r.emoji === emoji)
  if (count <= 0) return at < 0 ? [...list] : list.filter((_, i) => i !== at)
  if (at < 0) return [...list, { emoji, count, me: me ?? false }]
  return list.map((r, i) => (i === at ? { emoji, count, me: me ?? r.me } : r))
}

/** Tapping a chip (or a picker pick): the optimistic local toggle and what
 *  to ask the server (`on`: add the viewer's reaction). */
export function toggleReaction(
  messages: readonly HomeChatMessage[],
  msgId: string,
  emoji: string,
): { messages: HomeChatMessage[]; on: boolean } {
  const m = messages.find((x) => x.id === msgId)
  const cur = m?.reactions.find((r) => r.emoji === emoji)
  const on = !cur?.me
  if (!m) return { messages: messages as HomeChatMessage[], on }
  const count = (cur?.count ?? 0) + (on ? 1 : -1)
  return {
    messages: messages.map((x) => (x.id === msgId ? { ...x, reactions: withReaction(x.reactions, emoji, count, on) } : x)),
    on,
  }
}

/** A deleted message keeps its place with no words (Sew's tombstone). */
export function markDeleted(messages: readonly HomeChatMessage[], id: string): HomeChatMessage[] {
  if (!messages.some((m) => m.id === id)) return messages as HomeChatMessage[]
  return messages.map((m) => (m.id === id ? { ...m, deleted: true, body: '', reactions: [] } : m))
}

/** Member id → the name to show, from a server's people answer. */
export function memberNames(people: HomeChatPeople | null | undefined): Map<string, string> {
  const out = new Map<string, string>()
  for (const p of people?.people ?? []) if (p.memberId) out.set(p.memberId, p.displayName || p.username)
  return out
}

/** The name over a run or in a reply: "You" for the viewer, else the
 *  person's name, else "Someone" (a member K2 doesn't list). */
export function authorName(authorId: string, me: string | null, names: ReadonlyMap<string, string>): string {
  if (me && authorId === me) return 'You'
  return names.get(authorId) ?? 'Someone'
}

/** The first non-blank line of a message (the reply bar and quote strip). */
export function firstLine(text: string): string {
  return text.split('\n').find((l) => l.trim())?.trim() ?? ''
}

/** What a sent reply's quote strip says about its original (§11). */
export type ReplyPreview =
  | { state: 'loaded'; author: string; line: string }
  | { state: 'deleted' }
  | { state: 'missing' }

export function replyPreview(
  original: HomeChatMessage | undefined,
  me: string | null,
  names: ReadonlyMap<string, string>,
): ReplyPreview {
  if (!original) return { state: 'missing' }
  if (original.deleted) return { state: 'deleted' }
  return { state: 'loaded', author: authorName(original.authorId, me, names), line: firstLine(original.body) }
}

/** "Alex is typing…", "Alex and Sam are typing…", "Alex and 2 others are
 *  typing…" (§9.1); '' when nobody is. */
export function typingLine(names: readonly string[]): string {
  if (names.length === 0) return ''
  if (names.length === 1) return `${names[0]} is typing…`
  if (names.length === 2) return `${names[0]} and ${names[1]} are typing…`
  return `${names[0]} and ${names.length - 1} others are typing…`
}

/** A client nonce for one send: a retry of the same words sends the same
 *  one, so Sew never posts twice (§8.3). */
export function newNonce(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return crypto.randomUUID()
  return `n-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`
}

// ── This window's memory of each chat ──────────────────────────────────

const viewCache = new Map<string, HomeChatMessage[]>()

function cacheKey(hostKey: string, conv: string): string {
  return `${hostKey}|${conv}`
}

/** The chat as this window last drew it (newest VIEW_CACHE_MESSAGES). */
export function cachedHomeChat(hostKey: string, conv: string): HomeChatMessage[] | null {
  const hit = viewCache.get(cacheKey(hostKey, conv))
  if (!hit) return null
  // Most recently used last (eviction drops the first).
  viewCache.delete(cacheKey(hostKey, conv))
  viewCache.set(cacheKey(hostKey, conv), hit)
  return hit
}

export function rememberHomeChat(hostKey: string, conv: string, messages: readonly HomeChatMessage[]): void {
  const k = cacheKey(hostKey, conv)
  viewCache.delete(k)
  viewCache.set(k, messages.slice(-VIEW_CACHE_MESSAGES))
  while (viewCache.size > VIEW_CACHE_CHATS) {
    const oldest = viewCache.keys().next().value
    if (oldest === undefined) break
    viewCache.delete(oldest)
  }
}

export function resetHomeChatViewCacheForTests(): void {
  viewCache.clear()
}

// ── Emoji search (§10: Unicode only in v1) ─────────────────────────────

/** Unicode emoji with search words. Custom emoji are later. */
export const EMOJI_INDEX: ReadonlyArray<readonly [string, string]> = [
  ['👍', 'thumbs up yes ok like +1'], ['👎', 'thumbs down no dislike -1'], ['❤️', 'heart love red'],
  ['😂', 'joy laugh tears lol'], ['🎉', 'party tada celebrate congrats'], ['😮', 'wow surprised open mouth'],
  ['🙏', 'pray thanks please hands'], ['😀', 'grin smile happy'], ['😃', 'smile happy'], ['😄', 'smile happy eyes'],
  ['😁', 'beam grin'], ['😅', 'sweat smile relief'], ['🤣', 'rofl rolling laugh'], ['🙂', 'slight smile'],
  ['😉', 'wink'], ['😊', 'blush smile'], ['😍', 'heart eyes love'], ['🥰', 'hearts love smile'], ['😘', 'kiss'],
  ['😎', 'cool sunglasses'], ['🤔', 'thinking hmm'], ['🤨', 'raised eyebrow skeptical'], ['😐', 'neutral'],
  ['😑', 'expressionless'], ['🙄', 'eye roll'], ['😏', 'smirk'], ['😬', 'grimace awkward'], ['😴', 'sleep tired'],
  ['😪', 'sleepy'], ['😷', 'mask sick'], ['🤒', 'sick thermometer'], ['🤯', 'mind blown exploding'],
  ['🥳', 'party face celebrate'], ['😕', 'confused'], ['😟', 'worried'], ['😢', 'cry sad tear'], ['😭', 'sob crying'],
  ['😤', 'triumph huff'], ['😠', 'angry'], ['😡', 'rage pouting'], ['🤬', 'cursing'], ['😱', 'scream fear'],
  ['😳', 'flushed'], ['🥺', 'pleading'], ['🤗', 'hug'], ['🤭', 'oops hand over mouth'], ['🤫', 'shush quiet'],
  ['🤐', 'zipper mouth'], ['🫡', 'salute'], ['🫠', 'melting'], ['🙃', 'upside down'], ['😇', 'angel innocent'],
  ['🤓', 'nerd'], ['🧐', 'monocle'], ['😈', 'devil'], ['💀', 'skull dead'], ['👀', 'eyes look'], ['👋', 'wave hello bye'],
  ['👏', 'clap applause'], ['🙌', 'raised hands hooray'], ['💪', 'muscle strong'], ['🤝', 'handshake deal'],
  ['✌️', 'victory peace'], ['🤞', 'fingers crossed luck'], ['👌', 'ok perfect'], ['🤌', 'pinched'], ['👉', 'point right'],
  ['👈', 'point left'], ['👆', 'point up'], ['👇', 'point down'], ['☝️', 'index up'], ['✋', 'hand stop'],
  ['🖐️', 'hand fingers'], ['🤙', 'call me shaka'], ['💯', 'hundred 100 perfect'], ['🔥', 'fire hot lit'],
  ['✨', 'sparkles shiny'], ['⭐', 'star'], ['🌟', 'glowing star'], ['⚡', 'zap lightning'], ['💥', 'boom collision'],
  ['💡', 'idea bulb'], ['✅', 'check done yes'], ['☑️', 'check box'], ['✔️', 'check mark'], ['❌', 'cross no wrong'],
  ['❓', 'question'], ['❗', 'exclamation'], ['⚠️', 'warning'], ['🚀', 'rocket launch ship'], ['🎯', 'target bullseye'],
  ['🏆', 'trophy win'], ['🥇', 'gold medal first'], ['🎁', 'gift present'], ['🎂', 'cake birthday'], ['☕', 'coffee'],
  ['🍕', 'pizza'], ['🍺', 'beer'], ['🍻', 'cheers beers'], ['🥂', 'champagne toast'], ['🍿', 'popcorn'],
  ['🧡', 'orange heart'], ['💛', 'yellow heart'], ['💚', 'green heart'], ['💙', 'blue heart'], ['💜', 'purple heart'],
  ['🖤', 'black heart'], ['🤍', 'white heart'], ['💔', 'broken heart'], ['💕', 'two hearts'], ['💖', 'sparkling heart'],
  ['👻', 'ghost'], ['🤖', 'robot'], ['🐛', 'bug'], ['🐞', 'ladybug bug'], ['🦄', 'unicorn'], ['🐶', 'dog'], ['🐱', 'cat'],
  ['🌈', 'rainbow'], ['☀️', 'sun'], ['🌙', 'moon night'], ['❄️', 'snow cold'], ['🌊', 'wave water'],
  ['📌', 'pin pushpin'], ['📎', 'paperclip'], ['📝', 'memo note write'], ['📅', 'calendar date'], ['⏰', 'alarm clock'],
  ['⏳', 'hourglass wait'], ['🔒', 'lock'], ['🔑', 'key'], ['🛠️', 'tools fix'], ['🔧', 'wrench'], ['🧪', 'test tube'],
  ['📈', 'chart up growth'], ['📉', 'chart down'], ['💰', 'money bag'], ['🎵', 'music note'], ['🎶', 'music notes'],
  ['📣', 'megaphone announce'], ['🔔', 'bell'], ['💬', 'speech bubble chat'], ['👍🏻', 'thumbs up light'],
  ['🆗', 'ok button'], ['🆒', 'cool button'], ['➕', 'plus'], ['➖', 'minus'], ['🟢', 'green circle'], ['🔴', 'red circle'],
]

/** Emoji whose words start with every word of `q` (case-insensitive); the
 *  quick row's six when `q` is blank. */
export function searchEmoji(q: string, limit = 48): string[] {
  const words = q.trim().toLowerCase().split(/\s+/).filter(Boolean)
  if (words.length === 0) return [...QUICK_REACTIONS]
  const out: string[] = []
  for (const [emoji, keys] of EMOJI_INDEX) {
    const ks = keys.split(' ')
    if (words.every((w) => ks.some((k) => k.startsWith(w)))) out.push(emoji)
    if (out.length >= limit) break
  }
  return out
}

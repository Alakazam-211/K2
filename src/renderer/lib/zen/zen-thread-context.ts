// prd-garden-column-collapse-and-agent-list-v1 §4 (0.46.3 r10, CA6–CA8,
// CA14, CA16) — the Board context chip, renderer side.
//
// A custom widget (Baden's Board) calls `k2.conversation.setContext(text)`.
// The line is held here, in this window's memory, under the widget's view
// key (`<garden>/<placement>`), and drawn as a visible, removable chip in the
// message box of every Conversation that follows that widget: directly
// (`agents = "<placement>"`) or through an Agents widget's
// `list-from = "<placement>"`. It never is hidden text: what the chip shows
// is exactly what rides the person's next send, as its own `context` field,
// and the agent gets it on K2's own quoted line before the person's
// `[from]` stamp (the daemon writes that line; see overlay_routes.rs).
//
// The rule for the text is the daemon's (`check_context_text`, k2-core
// overlay/doc.rs): one line, at most 120 characters, no control or bidi
// characters, none of `[thread:`, `[from`, `[context`.
//
// Cleared by `setContext(null)`, a new text, the widget unloading or
// stopping (its layer's dispose), or a Garden switch (the frames unmount;
// the chip is looked up under the Garden on screen). After the person's ×
// the same text stays off; a different text shows again.

import { useSyncExternalStore } from 'react'
import type { ZenResolvedPage, ZenWidgetDecl } from './zen-page'
import type { ThreadContextBody } from '@/components/SessionView/overlayThread'
import type { ServerScope } from '@/kessel/server-scope'
import { checkZenWidgetCap } from './zen-widget-routes'

/** The `/boot-status` key of a server that takes a context. */
export const ZEN_THREAD_CONTEXT_FEATURE = 'thread-context-v1'
/** Longest context line, in characters. */
export const ZEN_THREAD_CONTEXT_MAX_CHARS = 120
/** `setContext` calls a widget may make in a second. */
export const ZEN_THREAD_CONTEXT_PER_SECOND = 2

const FORBIDDEN = ['[thread:', '[from', '[context'] as const

/** Line breaks and bidi controls: they change what the person sees. */
const BREAKS = new RegExp('[' + '\\u0000-\\u001f\\u007f-\\u009f\\u2028\\u2029\\u200e\\u200f\\u202a-\\u202e\\u2066-\\u2069' + ']')

/** Why `text` isn't a context line (the daemon's rule), or null. */
export function zenThreadContextProblem(text: string): string | null {
  const t = text.trim()
  if (!t) return 'A context is one line of text; use null to clear it.'
  if ([...t].length > ZEN_THREAD_CONTEXT_MAX_CHARS) return `A context is one line of at most ${ZEN_THREAD_CONTEXT_MAX_CHARS} characters.`
  if (BREAKS.test(t)) return 'A context is one line of plain text (no line breaks or control characters).'
  const lower = t.toLowerCase()
  const m = FORBIDDEN.find((f) => lower.includes(f))
  if (m) return `A context may not contain '${m}'.`
  return null
}

/** Where a context came from: the setting widget's port, never its frame. */
export interface ZenThreadContextOrigin {
  widget: string
  garden: string
  placement: string
  hash: string
}

export interface ZenThreadContext {
  text: string
  origin: ZenThreadContextOrigin
}

/** The view key a widget's context is held under. */
export function zenThreadContextKey(gardenId: string, placement: string): string {
  return `${gardenId}/${placement}`
}

function firstAgents(page: ZenResolvedPage): ZenWidgetDecl | null {
  let best: ZenWidgetDecl | null = null
  for (const w of page.widgets) if (w.kind === 'agents' && (best === null || w.column < best.column)) best = w
  return best
}

function prop(w: ZenWidgetDecl, name: string): string | null {
  const v = w.props[name]
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

/** The custom placement a Conversation widget follows for its context:
 *  the one it names (`agents`, UW40), or the `list-from` of the Agents
 *  widget it follows. Null for a pinned one (`agent`) or anything else. */
export function zenConversationContextSource(page: ZenResolvedPage, conversationId: string): string | null {
  const w = page.widgets.find((x) => x.id === conversationId)
  if (!w || w.kind !== 'conversation' || prop(w, 'agent')) return null
  const named = prop(w, 'agents')
  const custom = named ? page.widgets.find((x) => x.id === named && x.kind === 'custom') : undefined
  if (custom) return custom.id
  const follow = (named ? page.widgets.find((x) => x.id === named && x.kind === 'agents') : undefined) ?? firstAgents(page)
  const from = follow ? prop(follow, 'list-from') : null
  return from && page.widgets.some((x) => x.id === from && x.kind === 'custom') ? from : null
}

/** The Conversation widgets on `page` that follow `placement`. */
export function zenConversationsFollowing(page: ZenResolvedPage, placement: string): string[] {
  return page.widgets.filter((w) => zenConversationContextSource(page, w.id) === placement).map((w) => w.id)
}

// ── Sending ────────────────────────────────────────────────────────────────

/** A Board context refused before the post (another server's widget
 *  check): nothing was sent, the draft stays (§4: never lose the message). */
export class ZenContextRefusedError extends Error {
  readonly reason: string
  constructor(reason: string) {
    super(`The Board context wasn’t sent: ${reason}. Your message is still here; send again to send it without.`)
    this.name = 'ZenContextRefusedError'
    this.reason = reason
  }
}

/** What a send to the agent's server does with a context (CA6, CA8):
 * - a server without `thread-context-v1`: no context; `note` says so and
 *   the message goes without it (never folded into the body);
 * - this computer's daemon: the whole origin rides and it checks inline;
 * - another server: this computer's daemon checks the widget's
 *   `thread:post` first (`zen/widget/check`; a refusal throws
 *   `ZenContextRefusedError`), then that server gets the label and the
 *   placement (it doesn't hold the Garden). */
export async function zenThreadContextForServer(
  scope: Pick<ServerScope, 'isRemote' | 'serverSupports'>,
  serverName: string,
  ctx: ZenThreadContext,
): Promise<{ body?: ThreadContextBody; note?: string }> {
  if (!scope.serverSupports(ZEN_THREAD_CONTEXT_FEATURE)) return { note: `Not sent: ${serverName} is older` }
  const o = ctx.origin
  if (!scope.isRemote) return { body: { text: ctx.text, origin: { widget: o.widget, garden: o.garden, placement: o.placement, hash: o.hash } } }
  try {
    await checkZenWidgetCap({ garden: o.garden, placement: o.placement, widget: o.widget, hash: o.hash, cap: 'thread:post' })
  } catch (err) {
    throw new ZenContextRefusedError(err instanceof Error ? err.message : String(err))
  }
  return { body: { text: ctx.text, origin: { widget: o.widget, garden: o.garden, placement: o.placement } } }
}

// ── This window's chips ────────────────────────────────────────────────────

/** Source key → its context. */
const held = new Map<string, ZenThreadContext>()
/** The text the person removed with ×, per source key: stays off. */
const removed = new Map<string, string>()
/** Why the last context didn't go (shown where the chip was), per source key. */
const notes = new Map<string, string>()
/** `<garden>/<conversation>` → the source key it follows (set with the context). */
const followerIndex = new Map<string, string>()
const listeners = new Set<() => void>()
let version = 0

function changed(): void {
  version++
  for (const fn of [...listeners]) fn()
}

function unindex(key: string): void {
  for (const [f, k] of [...followerIndex]) if (k === key) followerIndex.delete(f)
}

/** Set (or with null, clear) a widget's context. `followers` are the
 *  Conversation widget ids that follow it on the page now. */
export function setZenThreadContext(key: string, ctx: ZenThreadContext | null, followers: readonly string[] = []): void {
  unindex(key)
  notes.delete(key)
  if (ctx === null) {
    held.delete(key)
    removed.delete(key)
  } else {
    held.set(key, ctx)
    for (const id of followers) followerIndex.set(zenThreadContextKey(ctx.origin.garden, id), key)
    if (removed.get(key) !== ctx.text) removed.delete(key)
  }
  changed()
}

/** A widget stopped or unloaded: its context goes (and any note of it). */
export function clearZenThreadContextFor(gardenId: string, placement: string): void {
  const key = zenThreadContextKey(gardenId, placement)
  if (!held.has(key) && !notes.has(key) && !removed.has(key)) return
  setZenThreadContext(key, null)
}

/** The person's × on the chip: this text stays off until a new one. */
export function removeZenThreadContext(key: string): void {
  const ctx = held.get(key)
  if (!ctx) return
  removed.set(key, ctx.text)
  changed()
}

/** A context that didn't go: the chip clears and says why. */
export function noteZenThreadContext(key: string, note: string): void {
  held.delete(key)
  removed.delete(key)
  notes.set(key, note)
  changed()
}

/** The person dismissed the note. */
export function clearZenThreadContextNote(key: string): void {
  if (!notes.delete(key)) return
  if (!held.has(key)) unindex(key)
  changed()
}

/** What a Conversation's message box shows: the source key, the chip (null
 *  when none or removed) and the last note. */
export interface ZenConversationContextView {
  key: string | null
  context: ZenThreadContext | null
  note: string | null
}

const NONE: ZenConversationContextView = { key: null, context: null, note: null }

export function zenConversationContext(gardenId: string, conversationId: string): ZenConversationContextView {
  if (!gardenId) return NONE
  const key = followerIndex.get(zenThreadContextKey(gardenId, conversationId))
  if (!key) return NONE
  const ctx = held.get(key)
  const shown = ctx && removed.get(key) !== ctx.text ? ctx : null
  const note = notes.get(key) ?? null
  if (!shown && note === null) return { key, context: null, note: null }
  return { key, context: shown, note }
}

/** The send path's check: the chip a Conversation shows now, if it is
 *  still `text` and its source is still what the page says it follows. */
export function zenContextForSend(
  page: ZenResolvedPage,
  gardenId: string,
  conversationId: string,
  text: string,
): { key: string; ctx: ZenThreadContext } | null {
  const view = zenConversationContext(gardenId, conversationId)
  if (!view.key || !view.context || view.context.text !== text) return null
  const source = zenConversationContextSource(page, conversationId)
  if (!source || zenThreadContextKey(gardenId, source) !== view.key) return null
  return { key: view.key, ctx: view.context }
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

/** React: the chip for one Conversation's box. */
export function useZenConversationContext(gardenId: string, conversationId: string): ZenConversationContextView {
  useSyncExternalStore(subscribe, () => version)
  return zenConversationContext(gardenId, conversationId)
}

/** Tests only. */
export function __resetZenThreadContextForTests(): void {
  held.clear()
  removed.clear()
  notes.clear()
  followerIndex.clear()
  changed()
}

// Rosson 2026-10-04 — the `app:navigate` bridge verbs behind Zen's nav rail
// (`nav-rail` widget, Garden 1's thin left rail).
//
//   bridge.call('app.open', 'home' | 'agents' | 'projects' | 'tickets')
//     Switches the Garden's VIEW in this window; Zen stays on (Rosson,
//     2026-10-04: Agents, Projects and Tickets open Zen versions of those
//     pages inside the Garden). `home` is the Garden's own page. It never
//     touches the app's page store: the page under Zen stays put. The view
//     is per window and remembered (`k2.zen.window.v1.<label>`).
//   bridge.call('app.current')               → the view shown now
//   bridge.call('app.subscribeCurrent', fn)  → unsubscribe; fn(view) on change
//   bridge.call('app.badges')                → { tickets, agents?, projects? }
//   bridge.call('app.subscribe', fn)         → unsubscribe; fn(badges) on change
//     The top bar's Tickets badge (`ticketsBadgeProps`), from the same
//     feedback store, so Zen and the top bar always agree. 0.46.1
//     (thread-read-v1): `agents` is this person's Thread badge (unmuted
//     unread + every card waiting on them) and `projects` the project
//     chats with something unread or waiting, from the unread store (the
//     window's server). Absent on a server without thread-read-v1 or at 0.
//     Numbers only.

import { ticketsBadgeProps } from '@/components/TopBar/PageTabs'
import { useFeedbackStore } from '@/stores/feedback'
import { selectProjectBadgeCount, useUnreadStore } from '@/stores/unread'
import { registerZenVerb, ZenBridgeError } from './zen-bridge'
import { isZenRailView, useZenWindowStore, ZEN_RAIL_VIEWS, type ZenRailView } from './zen-window'

export type ZenAppPage = ZenRailView

export const ZEN_APP_PAGES: readonly ZenAppPage[] = ZEN_RAIL_VIEWS

export interface ZenAppBadge {
  badge: number | '?'
  stale: boolean
  title: string | undefined
}

/** 0.46.1: a count badge from the unread store; `waiting` = some of it is
 *  cards waiting on you (drawn in the needs-you colour). */
export interface ZenAppCountBadge {
  badge: number
  waiting: boolean
}

export interface ZenAppBadges {
  tickets: ZenAppBadge
  agents?: ZenAppCountBadge
  projects?: ZenAppCountBadge
}

/** `app.open(page)`: show `page` in this window's Garden (Zen stays on). */
export function zenAppOpen(page: unknown): void {
  if (!isZenRailView(page)) {
    throw new ZenBridgeError('unknown_verb', 'app.open', `page must be one of ${ZEN_APP_PAGES.join(', ')}`)
  }
  useZenWindowStore.getState().setView(page)
}

/** `app.current()`: the view this window's Garden shows. */
export function zenAppCurrent(): ZenAppPage {
  return useZenWindowStore.getState().view
}

/** `app.subscribeCurrent(fn)`: `fn(view)` whenever it changes. */
export function zenAppSubscribeCurrent(fn: unknown): () => void {
  if (typeof fn !== 'function') throw new ZenBridgeError('unknown_verb', 'app.subscribeCurrent', 'needs a function')
  return useZenWindowStore.subscribe((s, prev) => {
    if (s.view !== prev.view) (fn as (v: ZenAppPage) => void)(s.view)
  })
}

/** The unread store's badges (window server; none before it loads). */
function unreadBadges(): Pick<ZenAppBadges, 'agents' | 'projects'> {
  const u = useUnreadStore.getState()
  if (!u.loaded) return {}
  const out: Pick<ZenAppBadges, 'agents' | 'projects'> = {}
  let threadWaiting = 0
  let projectWaiting = 0
  for (const e of Object.values(u.byKey)) {
    if (e.kind === 'thread') threadWaiting += e.waiting
    else projectWaiting += e.waiting
  }
  // The daemon's total also counts project chats; Agents shows Threads.
  let threadUnread = 0
  for (const e of Object.values(u.byKey)) if (e.kind === 'thread' && !e.muted) threadUnread += e.unread
  const agents = threadUnread + threadWaiting
  if (agents > 0) out.agents = { badge: agents, waiting: threadWaiting > 0 }
  const projects = selectProjectBadgeCount(u)
  if (projects > 0) out.projects = { badge: projects, waiting: projectWaiting > 0 }
  return out
}

/** `app.badges()`: the top bar's Tickets badge right now, and (0.46.1) the
 *  Agents and Projects unread badges. */
export function zenAppBadges(): ZenAppBadges {
  const s = useFeedbackStore.getState()
  const t = ticketsBadgeProps(s)
  return { tickets: { badge: t.badge, stale: t.badgeStale, title: t.badgeTitle }, ...unreadBadges() }
}

function sameCount(a: ZenAppCountBadge | undefined, b: ZenAppCountBadge | undefined): boolean {
  return a?.badge === b?.badge && a?.waiting === b?.waiting
}

function sameBadges(a: ZenAppBadges, b: ZenAppBadges): boolean {
  return (
    a.tickets.badge === b.tickets.badge &&
    a.tickets.stale === b.tickets.stale &&
    a.tickets.title === b.tickets.title &&
    sameCount(a.agents, b.agents) &&
    sameCount(a.projects, b.projects)
  )
}

/** `app.subscribe(fn)`: `fn(badges)` whenever they change. */
export function zenAppSubscribe(fn: unknown): () => void {
  if (typeof fn !== 'function') throw new ZenBridgeError('unknown_verb', 'app.subscribe', 'needs a function')
  let last = zenAppBadges()
  const check = (): void => {
    const next = zenAppBadges()
    if (sameBadges(next, last)) return
    last = next
    ;(fn as (b: ZenAppBadges) => void)(next)
  }
  const offFeedback = useFeedbackStore.subscribe(check)
  const offUnread = useUnreadStore.subscribe(check)
  return () => {
    offFeedback()
    offUnread()
  }
}

/** Register the `app:navigate` verbs. Returns the uninstall. */
export function installZenAppNavVerbs(): () => void {
  const offs = [
    registerZenVerb('app.open', (_ctx, page) => zenAppOpen(page)),
    registerZenVerb('app.current', () => zenAppCurrent()),
    registerZenVerb('app.subscribeCurrent', (_ctx, fn) => zenAppSubscribeCurrent(fn)),
    registerZenVerb('app.badges', () => zenAppBadges()),
    registerZenVerb('app.subscribe', (_ctx, fn) => zenAppSubscribe(fn)),
  ]
  return () => {
    for (const off of offs) off()
  }
}

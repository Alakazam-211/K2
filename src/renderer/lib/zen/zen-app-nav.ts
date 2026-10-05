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
//   bridge.call('app.badges')                → { tickets: ZenAppBadge }
//   bridge.call('app.subscribe', fn)         → unsubscribe; fn(badges) on change
//     The top bar's Tickets badge (`ticketsBadgeProps`), from the same
//     feedback store, so Zen and the top bar always agree.

import { ticketsBadgeProps } from '@/components/TopBar/PageTabs'
import { useFeedbackStore } from '@/stores/feedback'
import { registerZenVerb, ZenBridgeError } from './zen-bridge'
import { isZenRailView, useZenWindowStore, ZEN_RAIL_VIEWS, type ZenRailView } from './zen-window'

export type ZenAppPage = ZenRailView

export const ZEN_APP_PAGES: readonly ZenAppPage[] = ZEN_RAIL_VIEWS

export interface ZenAppBadge {
  badge: number | '?'
  stale: boolean
  title: string | undefined
}

export interface ZenAppBadges {
  tickets: ZenAppBadge
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

/** `app.badges()`: the top bar's Tickets badge right now. */
export function zenAppBadges(): ZenAppBadges {
  const s = useFeedbackStore.getState()
  const t = ticketsBadgeProps(s)
  return { tickets: { badge: t.badge, stale: t.badgeStale, title: t.badgeTitle } }
}

function sameBadges(a: ZenAppBadges, b: ZenAppBadges): boolean {
  return a.tickets.badge === b.tickets.badge && a.tickets.stale === b.tickets.stale && a.tickets.title === b.tickets.title
}

/** `app.subscribe(fn)`: `fn(badges)` whenever they change. */
export function zenAppSubscribe(fn: unknown): () => void {
  if (typeof fn !== 'function') throw new ZenBridgeError('unknown_verb', 'app.subscribe', 'needs a function')
  let last = zenAppBadges()
  return useFeedbackStore.subscribe(() => {
    const next = zenAppBadges()
    if (sameBadges(next, last)) return
    last = next
    ;(fn as (b: ZenAppBadges) => void)(next)
  })
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

// Rosson 2026-10-04 — the `app:navigate` bridge verbs behind Zen's nav rail
// (`nav-rail` widget, Garden 1's thin left rail).
//
//   bridge.call('app.open', 'agents' | 'projects' | 'tickets')
//     Switching to an app page exits Zen (Rosson's rule): this window's Zen
//     goes off through the same path as the toggle and ⌃⌘Z (`exitZen`),
//     Settings closes, and the page store selects the page, exactly like the
//     top bar's page tabs. `'home'` is the Garden itself: nothing happens.
//   bridge.call('app.badges')            → { tickets: ZenAppBadge }
//   bridge.call('app.subscribe', fn)     → unsubscribe; fn(badges) on change
//     The top bar's Tickets badge (`ticketsBadgeProps`), from the same
//     feedback store, so Zen and the top bar always agree.

import { ticketsBadgeProps } from '@/components/TopBar/PageTabs'
import { useFeedbackStore } from '@/stores/feedback'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { registerZenVerb, ZenBridgeError } from './zen-bridge'
import { exitZen } from './zen-view'

export type ZenAppPage = 'home' | 'agents' | 'projects' | 'tickets'

export const ZEN_APP_PAGES: readonly ZenAppPage[] = ['home', 'agents', 'projects', 'tickets']

const APP_PAGE: Record<Exclude<ZenAppPage, 'home'>, AppPage> = {
  agents: 'agents',
  projects: 'projects',
  tickets: 'feedback',
}

export interface ZenAppBadge {
  badge: number | '?'
  stale: boolean
  title: string | undefined
}

export interface ZenAppBadges {
  tickets: ZenAppBadge
}

/** `app.open(page)`: leave Zen in this window and open `page`. */
export function zenAppOpen(page: unknown): void {
  if (typeof page !== 'string' || !(ZEN_APP_PAGES as readonly string[]).includes(page)) {
    throw new ZenBridgeError('unknown_verb', 'app.open', `page must be one of ${ZEN_APP_PAGES.join(', ')}`)
  }
  if (page === 'home') return
  exitZen()
  if (useSettingsStore.getState().settingsOpen) useSettingsStore.getState().closeSettings()
  usePageViewStore.getState().setPage(APP_PAGE[page as Exclude<ZenAppPage, 'home'>])
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
    registerZenVerb('app.badges', () => zenAppBadges()),
    registerZenVerb('app.subscribe', (_ctx, fn) => zenAppSubscribe(fn)),
  ]
  return () => {
    for (const off of offs) off()
  }
}

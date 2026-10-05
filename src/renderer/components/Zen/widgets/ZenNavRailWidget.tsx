// Rosson 2026-10-04 — Garden 1's thin left rail (`nav-rail` widget): four
// icon buttons, top to bottom, with the top bar's page names as tooltips.
// Each switches the Garden's view in this window, inside Zen (Zen stays
// on; the page under Zen never changes):
//
//   My Home   the Garden's own page: the texting view of your Homes;
//   Agents    the same texting view of this server's agents (focus groups);
//   Projects  Zen's Projects view (coming soon);
//   Tickets   Zen's Tickets view, with the top bar's waiting badge.
//
// The view shown is the current item (`aria-current`). ZenPage draws a rail
// kind at the left edge of its column, outside the column's box. About
// 44px wide, icons only. Everything goes through the bridge
// (`app:navigate`: `app.open`, `app.current`, `app.subscribeCurrent`,
// `app.badges`, `app.subscribe`).
//
// Rosson 2026-10-04, "Option A": the current item sits on one liquid glass
// pill that springs from icon to icon when the view changes. The slide is
// Motion's shared layout (`layoutId`, scoped per rail so two rails never
// trade pills); the glass is plain CSS that WebKit draws (no SVG filters in
// `backdrop-filter`): a low-alpha accent tint over the surface, backdrop
// blur + saturate, a light rim, a top highlight, a soft shadow; the inside
// is flat (no sheen gradient, Rosson 2026-10-04). The rail under it is a shared Zen glass tile, like every
// other Zen tile. Colours are Zen tokens only. Reduced motion: no slide,
// the pill just appears on the new item. Reduced transparency: a solid
// tint, no blur, no sheen.

import { useEffect, useId, useState } from 'react'
import { LayoutGroup, motion, type Transition } from 'motion/react'
import { badgeText, PAGE_TAB_LABELS } from '@/components/TopBar/PageTabs'
import type { ZenAppBadges, ZenAppPage } from '@/lib/zen/zen-app-nav'
import { ZEN_GLASS_PROPS } from '@/lib/zen/zen-glass'
import { REDUCED_MOTION_QUERY, zenMediaMatches } from '@/lib/zen/zen-theme'
import type { ZenWidgetProps } from '../zen-registry'

/** The rail's width (CSS px). */
export const ZEN_NAV_RAIL_WIDTH_PX = 44

/** The pill's spring: quick, settles with a hint of give, no wobble. */
export const ZEN_NAV_PILL_SPRING: Transition = { type: 'spring', stiffness: 500, damping: 35, mass: 1 }

/** How the pill moves: a shared-layout spring, or (reduced motion) no
 *  shared layout at all, so it is simply drawn on the new item. */
export function zenNavPillMotion(
  reducedMotion: boolean,
  layoutId: string,
): { mode: 'spring' | 'instant'; layoutId: string | undefined; transition: Transition } {
  return reducedMotion
    ? { mode: 'instant', layoutId: undefined, transition: { duration: 0 } }
    : { mode: 'spring', layoutId, transition: ZEN_NAV_PILL_SPRING }
}

// Glass recipe. Light: a white rim + bright top highlight; dark: both
// dimmer, and a deeper shadow. Every colour comes from a Zen token or plain
// white/black at low alpha, so each Zen theme tints it.
const PILL_TINT = 'color-mix(in srgb, var(--zen-accent) 16%, color-mix(in srgb, var(--zen-surface-raised) 52%, transparent))'

/** The rail's styles: the glass pill and the hover glow. The rail itself is
 *  a shared Zen glass tile (`data-zen-glass`, `lib/zen/zen-glass.ts`). */
export const ZEN_NAV_RAIL_CSS = `
[data-zen-root] [data-zen-widget="nav-rail"] {
  --zen-nav-pill-tint: ${PILL_TINT};
  --zen-nav-pill-rim: color-mix(in srgb, var(--zen-accent) 22%, rgb(255 255 255 / 0.62));
  --zen-nav-pill-highlight: rgb(255 255 255 / 0.75);
  --zen-nav-pill-shadow: 0 1px 2px rgb(0 0 0 / 0.06), 0 4px 12px color-mix(in srgb, var(--zen-accent) 18%, transparent);
}
[data-zen-root][data-zen-scheme="dark"] [data-zen-widget="nav-rail"] {
  --zen-nav-pill-tint: color-mix(in srgb, var(--zen-accent) 22%, color-mix(in srgb, var(--zen-surface-raised) 48%, transparent));
  --zen-nav-pill-rim: color-mix(in srgb, var(--zen-accent) 26%, rgb(255 255 255 / 0.16));
  --zen-nav-pill-highlight: rgb(255 255 255 / 0.22);
  --zen-nav-pill-shadow: 0 1px 2px rgb(0 0 0 / 0.35), 0 6px 16px rgb(0 0 0 / 0.30);
}
[data-zen-root] [data-zen-nav-pill] {
  background: var(--zen-nav-pill-tint); /* flat inside; no sheen gradient (Rosson 2026-10-04) */
  -webkit-backdrop-filter: none; /* no blur: stale WebKit layers after a theme switch */
  backdrop-filter: none;
  border: 1px solid var(--zen-nav-pill-rim);
}
[data-zen-root] [data-zen-nav] { transition: color 160ms ease, background-color 160ms ease, box-shadow 160ms ease; }
[data-zen-root] [data-zen-nav]:not([aria-current]):hover {
  color: var(--zen-text);
  background: color-mix(in srgb, var(--zen-accent) 9%, transparent);
  box-shadow: 0 0 10px color-mix(in srgb, var(--zen-accent) 16%, transparent);
}
[data-zen-root] [data-zen-nav]:focus-visible { outline: 2px solid color-mix(in srgb, var(--zen-accent) 60%, transparent); outline-offset: 1px; }
@media (prefers-reduced-motion: reduce) {
  [data-zen-root] [data-zen-nav] { transition: none; }
}
@media (prefers-reduced-transparency: reduce) {
  [data-zen-root] [data-zen-nav-pill] {
    background: color-mix(in srgb, var(--zen-accent) 14%, var(--zen-surface-raised));
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
    border-color: color-mix(in srgb, var(--zen-accent) 30%, var(--zen-border));
  }
}
`

const ICON = {
  width: 18,
  height: 18,
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.7,
  strokeLinecap: 'round' as const,
  strokeLinejoin: 'round' as const,
  'aria-hidden': true,
}

function HomeIcon(): React.JSX.Element {
  return (
    <svg {...ICON}>
      <path d="M3 10.5 12 3l9 7.5" />
      <path d="M5 9.5V20h5v-6h4v6h5V9.5" />
    </svg>
  )
}

function AgentIcon(): React.JSX.Element {
  return (
    <svg {...ICON}>
      <rect x="4" y="7" width="16" height="12" rx="3" />
      <path d="M12 3v4" />
      <circle cx="9" cy="13" r="1" />
      <circle cx="15" cy="13" r="1" />
      <path d="M2 12v3M22 12v3" />
    </svg>
  )
}

function DashboardIcon(): React.JSX.Element {
  return (
    <svg {...ICON}>
      <rect x="3" y="3" width="7" height="9" rx="1" />
      <rect x="14" y="3" width="7" height="5" rx="1" />
      <rect x="14" y="12" width="7" height="9" rx="1" />
      <rect x="3" y="16" width="7" height="5" rx="1" />
    </svg>
  )
}

function TicketIcon(): React.JSX.Element {
  return (
    <svg {...ICON}>
      <path d="M3 8a2 2 0 0 0 2-2h14a2 2 0 0 0 2 2v2a2 2 0 0 0 0 4v2a2 2 0 0 0-2 2H5a2 2 0 0 0-2-2v-2a2 2 0 0 0 0-4Z" />
      <path d="M14 6v2M14 11v2M14 16v2" />
    </svg>
  )
}

interface RailItem {
  page: ZenAppPage
  name: string
  title: string
  Icon: () => React.JSX.Element
}

export const ZEN_NAV_RAIL_ITEMS: readonly RailItem[] = [
  { page: 'home', name: PAGE_TAB_LABELS.home.name, title: `${PAGE_TAB_LABELS.home.name} — the texting view of your Homes`, Icon: HomeIcon },
  { page: 'agents', name: PAGE_TAB_LABELS.agents.name, title: `${PAGE_TAB_LABELS.agents.name} — this server’s agents, in Zen`, Icon: AgentIcon },
  { page: 'projects', name: PAGE_TAB_LABELS.projects.name, title: `${PAGE_TAB_LABELS.projects.name} — in Zen`, Icon: DashboardIcon },
  { page: 'tickets', name: PAGE_TAB_LABELS.feedback.name, title: `${PAGE_TAB_LABELS.feedback.title}, in Zen`, Icon: TicketIcon },
]

function useBadges(bridge: ZenWidgetProps['bridge']): ZenAppBadges {
  const [badges, setBadges] = useState<ZenAppBadges>(() => bridge.call('app.badges') as ZenAppBadges)
  useEffect(() => {
    setBadges(bridge.call('app.badges') as ZenAppBadges)
    const off = bridge.call('app.subscribe', (next: ZenAppBadges) => setBadges(next))
    if (typeof off !== 'function') throw new Error('zen: app.subscribe returned no unsubscribe')
    return off as () => void
  }, [bridge])
  return badges
}

/** The view this window's Garden shows, live (`app.current`). */
function useCurrentView(bridge: ZenWidgetProps['bridge']): ZenAppPage {
  const [view, setView] = useState<ZenAppPage>(() => bridge.call('app.current') as ZenAppPage)
  useEffect(() => {
    setView(bridge.call('app.current') as ZenAppPage)
    const off = bridge.call('app.subscribeCurrent', (next: ZenAppPage) => setView(next))
    if (typeof off !== 'function') throw new Error('zen: app.subscribeCurrent returned no unsubscribe')
    return off as () => void
  }, [bridge])
  return view
}

/** `prefers-reduced-motion`, live. */
function useReducedMotion(): boolean {
  const [on, setOn] = useState(() => zenMediaMatches(REDUCED_MOTION_QUERY))
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia(REDUCED_MOTION_QUERY)
    const sync = (): void => setOn(mq.matches)
    sync()
    mq.addEventListener?.('change', sync)
    return () => mq.removeEventListener?.('change', sync)
  }, [])
  return on
}

/** The glass pill behind the current icon. One per rail. */
function SelectionPill({ scope, reducedMotion }: { scope: string; reducedMotion: boolean }): React.JSX.Element {
  const m = zenNavPillMotion(reducedMotion, 'zen-nav-pill')
  return (
    <motion.div
      aria-hidden
      data-zen-nav-pill=""
      data-zen-nav-pill-motion={m.mode}
      data-zen-nav-pill-scope={scope}
      layoutId={m.layoutId}
      transition={m.transition}
      initial={false}
      style={{
        position: 'absolute',
        inset: 0,
        zIndex: 0,
        borderRadius: 'calc(var(--zen-radius) - 4px)',
        boxShadow: 'inset 0 1px 0 var(--zen-nav-pill-highlight), inset 0 -1px 0 rgb(255 255 255 / 0.06), var(--zen-nav-pill-shadow)',
        pointerEvents: 'none',
      }}
    />
  )
}

export function ZenNavRailWidget({ bridge, decl }: ZenWidgetProps): React.JSX.Element {
  const badges = useBadges(bridge)
  const view = useCurrentView(bridge)
  const reducedMotion = useReducedMotion()
  const tickets = badgeText(badges.tickets.badge)
  // Scope the shared layout to this rail: two rails (or a remount next to
  // the old one) never animate each other's pill.
  const scope = `zen-nav-rail-${decl.id}-${useId()}`
  return (
    <LayoutGroup id={scope}>
      <nav
        aria-label="Pages"
        data-zen-widget="nav-rail"
        data-zen-widget-id={decl.id}
        {...ZEN_GLASS_PROPS}
        className="flex flex-shrink-0 flex-col items-center"
        style={{
          width: ZEN_NAV_RAIL_WIDTH_PX,
          padding: '6px 0',
          gap: 4,
          borderRadius: 'var(--zen-radius)',
        }}
      >
        <style data-zen-nav-rail-glass="">{ZEN_NAV_RAIL_CSS}</style>
        {ZEN_NAV_RAIL_ITEMS.map(({ page, name, title, Icon }) => {
          const current = page === view
          return (
            <button
              key={page}
              type="button"
              aria-label={name}
              aria-current={current ? 'page' : undefined}
              title={title}
              data-zen-nav={page}
              onClick={() => {
                if (current) return
                try {
                  bridge.call('app.open', page)
                } catch (err) {
                  console.warn(`[zen] open ${page} failed:`, err)
                }
              }}
              className={`no-drag relative flex items-center justify-center ${current ? 'cursor-default' : 'cursor-pointer'}`}
              style={{
                width: 32,
                height: 32,
                borderRadius: 'calc(var(--zen-radius) - 4px)',
                color: current ? 'var(--zen-accent)' : 'var(--zen-text-muted)',
              }}
            >
              {current && <SelectionPill scope={scope} reducedMotion={reducedMotion} />}
              <span className="relative flex items-center justify-center" style={{ zIndex: 1 }}>
                <Icon />
              </span>
              {page === 'tickets' && tickets !== null && (
                <span
                  data-zen-nav-badge=""
                  data-stale={badges.tickets.stale ? 'true' : 'false'}
                  title={badges.tickets.title}
                  className="absolute flex items-center justify-center"
                  style={{
                    zIndex: 2,
                    top: -2,
                    right: -2,
                    minWidth: 14,
                    height: 14,
                    padding: '0 3px',
                    borderRadius: 999,
                    fontSize: 8,
                    fontWeight: 700,
                    background: 'var(--zen-working)',
                    color: 'var(--zen-accent-text)',
                    opacity: badges.tickets.stale ? 0.5 : 1,
                  }}
                >
                  {tickets}
                </span>
              )}
            </button>
          )
        })}
      </nav>
    </LayoutGroup>
  )
}

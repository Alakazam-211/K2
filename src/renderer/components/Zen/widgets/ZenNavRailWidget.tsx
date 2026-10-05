// Rosson 2026-10-04 — Garden 1's thin left rail (`nav-rail` widget): four
// icon buttons, top to bottom, with the top bar's page names as tooltips:
//
//   My Home   current: this Garden is the texting view of your Homes;
//   Agents    leave Zen in this window and open the Agents page;
//   Projects  … the Projects page;
//   Tickets   … the Tickets page, with the top bar's waiting badge.
//
// ZenPage draws a rail kind at the left edge of its column, outside the
// column's box. About 44px wide, icons only. Everything goes through the
// bridge (`app:navigate`: `app.open`, `app.badges`, `app.subscribe`).

import { useEffect, useState } from 'react'
import { badgeText, PAGE_TAB_LABELS } from '@/components/TopBar/PageTabs'
import type { ZenAppBadges, ZenAppPage } from '@/lib/zen/zen-app-nav'
import type { ZenWidgetProps } from '../zen-registry'

/** The rail's width (CSS px). */
export const ZEN_NAV_RAIL_WIDTH_PX = 44

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
  { page: 'home', name: PAGE_TAB_LABELS.home.name, title: `${PAGE_TAB_LABELS.home.name} — you're here: this Garden is the texting view of your Homes`, Icon: HomeIcon },
  { page: 'agents', name: PAGE_TAB_LABELS.agents.name, title: `${PAGE_TAB_LABELS.agents.title}. Leaves Zen in this window.`, Icon: AgentIcon },
  { page: 'projects', name: PAGE_TAB_LABELS.projects.name, title: `${PAGE_TAB_LABELS.projects.title}. Leaves Zen in this window.`, Icon: DashboardIcon },
  { page: 'tickets', name: PAGE_TAB_LABELS.feedback.name, title: `${PAGE_TAB_LABELS.feedback.title}. Leaves Zen in this window.`, Icon: TicketIcon },
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

export function ZenNavRailWidget({ bridge, decl }: ZenWidgetProps): React.JSX.Element {
  const badges = useBadges(bridge)
  const tickets = badgeText(badges.tickets.badge)
  return (
    <nav
      aria-label="Pages"
      data-zen-widget="nav-rail"
      data-zen-widget-id={decl.id}
      className="flex flex-shrink-0 flex-col items-center"
      style={{
        width: ZEN_NAV_RAIL_WIDTH_PX,
        padding: '6px 0',
        gap: 4,
        background: 'var(--zen-surface)',
        border: '1px solid var(--zen-border)',
        borderRadius: 'var(--zen-radius)',
      }}
    >
      {ZEN_NAV_RAIL_ITEMS.map(({ page, name, title, Icon }) => {
        const current = page === 'home'
        return (
          <button
            key={page}
            type="button"
            aria-label={name}
            aria-current={current ? 'page' : undefined}
            title={title}
            data-zen-nav={page}
            data-zen-soft-button=""
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
              background: current ? 'var(--zen-surface-raised)' : 'transparent',
              boxShadow: current ? 'inset 2px 0 0 var(--zen-accent)' : undefined,
            }}
          >
            <Icon />
            {page === 'tickets' && tickets !== null && (
              <span
                data-zen-nav-badge=""
                data-stale={badges.tickets.stale ? 'true' : 'false'}
                title={badges.tickets.title}
                className="absolute flex items-center justify-center"
                style={{
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
  )
}

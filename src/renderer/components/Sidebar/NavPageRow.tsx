// 0.46.0 UI experiment (Rosson 2026-10-16, may be reversed): the page
// switcher's Home / Agents / Projects / Tickets / Gardens / Settings as
// icon-only buttons along the top of the nav bar, using the Garden nav
// rail's icons (Gardens is the Zen toggle's ensō; Settings is the app's
// cog, which Gardens draw too). Gardens shows only where Zen exists. The same row
// sits at the top of the left nav on all four pages (Home and Agents
// sidebars, the Projects nav, the Tickets list), expanded or collapsed.
// The top-bar tabs stay as they are for now.
//
// Each button is a `PageTab` (the cog's look: size, hover, selected
// underline) and runs the top-bar tab's action (`selectPage`). Badges
// mirror the top-bar tabs (Projects unread, Tickets waiting); the top bar
// owns the event wiring and the waiting re-count, this row only reads the
// stores. The top-bar tabs have no per-server visibility rules, so none
// here either.
//
// `orientation="row"` (default) for an expanded nav; `"column"` stacks the
// four buttons for the 48px collapsed rails.

import type { AppPage } from '@/stores/page-view'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useFeedbackStore } from '@/stores/feedback'
import { useProjectGroupsStore } from '@/stores/project-groups'
import {
  openSettingsTab,
  PAGE_TAB_LABELS,
  PageTab,
  selectPage,
  SettingsCogIcon,
  ticketsBadgeProps,
} from '@/components/TopBar/PageTabs'
import { withChord } from '@/lib/shortcut-label'
import ZenTopBarToggle from '@/components/TopBar/ZenTopBarToggle'
import {
  GardenAgentIcon,
  GardenDashboardIcon,
  GardenHomeIcon,
  GardenTicketIcon,
} from '@/components/icons/GardenPageIcons'

/** Icon size in the row: the top-bar cog's 13px. */
export const NAV_PAGE_ICON_PX = 13

interface NavPageItem {
  page: AppPage
  name: string
  title: string
  Icon: typeof GardenHomeIcon
}

export const NAV_PAGE_ITEMS: readonly NavPageItem[] = [
  { page: 'home', name: PAGE_TAB_LABELS.home.name, title: PAGE_TAB_LABELS.home.title, Icon: GardenHomeIcon },
  { page: 'agents', name: PAGE_TAB_LABELS.agents.name, title: PAGE_TAB_LABELS.agents.title, Icon: GardenAgentIcon },
  { page: 'projects', name: PAGE_TAB_LABELS.projects.name, title: PAGE_TAB_LABELS.projects.title, Icon: GardenDashboardIcon },
  { page: 'feedback', name: PAGE_TAB_LABELS.feedback.name, title: PAGE_TAB_LABELS.feedback.title, Icon: GardenTicketIcon },
]

export default function NavPageRow({
  orientation = 'row',
}: {
  orientation?: 'row' | 'column'
}): React.JSX.Element {
  const page = usePageViewStore((s) => s.page)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const waitingCount = useFeedbackStore((s) => s.waitingCount)
  const waitingStale = useFeedbackStore((s) => s.waitingStale)
  const waitingUnsupported = useFeedbackStore((s) => s.waitingUnsupported)
  const ticketsBadge = ticketsBadgeProps({ waitingCount, waitingStale, waitingUnsupported })
  const projectsUnread = useProjectGroupsStore((s) => s.unreadGroupIds.size)
  const column = orientation === 'column'

  return (
    <nav
      aria-label="Pages"
      aria-orientation={column ? 'vertical' : 'horizontal'}
      data-nav-page-row={orientation}
      className={`no-drag flex flex-shrink-0 items-center gap-0.5 border-b border-[var(--color-border)] ${
        column ? 'flex-col w-full pb-1.5 mb-1.5' : 'flex-row justify-around px-2 py-1.5'
      }`}
    >
      {NAV_PAGE_ITEMS.map(({ page: p, name, title, Icon }) => {
        const badge =
          p === 'projects'
            ? { badge: projectsUnread }
            : p === 'feedback'
              ? {
                  badge: ticketsBadge.badge,
                  badgeClass: 'bg-[var(--color-status-working)]',
                  badgeStale: ticketsBadge.badgeStale,
                  badgeTitle: ticketsBadge.badgeTitle,
                }
              : {}
        return (
          <PageTab
            key={p}
            selected={!settingsOpen && page === p}
            current
            onSelect={() => selectPage(p)}
            title={title}
            ariaLabel={name}
            {...badge}
          >
            <Icon size={NAV_PAGE_ICON_PX} />
          </PageTab>
        )
      })}
      {/* Gardens (Rosson 2026-10-16): the Zen toggle, moved here from the
          top bar. Renders nothing where Zen doesn't exist (web, Windows,
          Focus and ticket windows), so the row is five icons there. */}
      <ZenTopBarToggle />
      {/* Settings last (Rosson 2026-10-16): the top bar's cog, same action. */}
      <PageTab
        selected={settingsOpen}
        current
        onSelect={openSettingsTab}
        title={withChord('Settings', 'Meta+,')}
        ariaLabel="Settings"
      >
        <SettingsCogIcon size={NAV_PAGE_ICON_PX} />
      </PageTab>
    </nav>
  )
}

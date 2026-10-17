// Projects V1 P4 (prd-projects-v1 §6.0) — the top bar's page switcher:
// ⚙ | Home | Agents | Projects | Tickets | Wiki. Replaces the standalone
// v0.40.26 FeedbackTopBarButton: its waiting-count badge moves onto the
// Feedback tab (same count, same show/hide semantics) and its event
// wiring (initFeedbackEvents + the badge re-count on projects changes)
// moves here unchanged. The Projects tab carries the unread-groups badge
// (§4.4). The settings cog is a fourth tab with the same selected
// underline. Rendered in the workspace TopBar, overlay pages, AND
// Settings so the switcher stays available while Settings is open.

import { useEffect, type ReactNode } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { useFeedbackStore, initFeedbackEvents } from '@/stores/feedback'
import { useProjectGroupsStore, initProjectGroupEvents } from '@/stores/project-groups'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { withChord } from '@/lib/shortcut-label'
import {
  AgentsIcon,
  HomeIcon,
  K2_ICON_PX,
  ProjectsIcon,
  SettingsIcon,
  TicketsIcon,
  WikiIcon,
} from '@/components/icons/K2Icons'
import { openWikiForView } from '@/lib/wiki-open'
import { pageShortcutLabel } from '@/lib/page-shortcuts'
import { isMacPlatform } from '@/lib/desktop-chrome'

/** The Tickets tab's badge from the feedback store (prd-tickets-badge-
 *  orphans TB8/TB15/TB17 + Appa A3): a fresh count; the last count dimmed
 *  when it may be out of date; a dimmed `?` on a server too old to count. */
export function ticketsBadgeProps(s: {
  waitingCount: number
  waitingStale: boolean
  waitingUnsupported: boolean
}): { badge: number | '?'; badgeStale: boolean; badgeTitle: string | undefined } {
  return {
    badge: s.waitingUnsupported ? '?' : s.waitingCount,
    badgeStale: s.waitingUnsupported || s.waitingStale,
    badgeTitle: s.waitingUnsupported
      ? 'This server is too old to count tickets'
      : s.waitingStale
        ? 'Tickets count may be out of date'
        : undefined,
  }
}

/** The page tabs' names and tooltips (also Zen's nav rail). */
export const PAGE_TAB_LABELS = {
  home: { name: 'My Home', title: 'My Home — your own page on this computer: your agents from any server. Other people on the server don\'t see it.' },
  agents: { name: 'Agents', title: 'Agents — the workspace view' },
  projects: { name: 'Projects', title: 'Projects — grouped agents, shared dashboards' },
  feedback: { name: 'Tickets', title: 'Tickets — agents waiting on a human' },
  wiki: { name: 'Wiki', title: 'Wiki' },
} as const

/** 0.46.0 experiment (Rosson 2026-10-16): the page tabs are icons, not
 *  words. The word stays as the tooltip and the accessible name. Icon
 *  style A3 (`components/icons/K2Icons.tsx`): one 16px size for every
 *  top-bar icon, the cog included, inside the 24px-tall tab. */
export const PAGE_TAB_ICON_PX = K2_ICON_PX

/** The five page tabs in order, with their A3 icons. Wiki (0.46.0, Rosson
 *  2026-10-16) opens the wiki of the workspace on screen
 *  (`lib/wiki-open.ts`), not a bare page switch. */
export const PAGE_TAB_ITEMS = [
  { page: 'home', Icon: HomeIcon },
  { page: 'agents', Icon: AgentsIcon },
  { page: 'projects', Icon: ProjectsIcon },
  { page: 'feedback', Icon: TicketsIcon },
  { page: 'wiki', Icon: WikiIcon },
] as const satisfies readonly { page: keyof typeof PAGE_TAB_LABELS; Icon: typeof HomeIcon }[]

/** A page tab's tooltip with its chords: the page shortcut (⌃⌘1–4,
 *  Ctrl+Alt+1–4 off macOS, or the rebound one) and ⌘P for Projects. */
export function pageTabTitle(page: keyof typeof PAGE_TAB_LABELS, keybindings: Record<string, string>, isMac: boolean): string {
  const chords = [pageShortcutLabel(page, keybindings, isMac), page === 'projects' ? '⌘P' : null].filter(
    (c): c is string => !!c,
  )
  const title = PAGE_TAB_LABELS[page].title
  return chords.length > 0 ? `${title} (${chords.join(', ')})` : title
}

/** What a tab badge shows: nothing, a number, or `?` (unknown). */
export function badgeText(badge: number | '?' | undefined): string | null {
  if (badge === '?') return '?'
  if (badge === undefined || badge <= 0) return null
  return badge > 99 ? '99+' : String(badge)
}

export function PageTab({
  selected,
  onSelect,
  badge,
  badgeClass,
  badgeStale = false,
  badgeTitle,
  title,
  ariaLabel,
  children,
}: {
  selected: boolean
  onSelect: () => void
  /** A count (0 hides) or `?` when the server cannot give one. */
  badge?: number | '?'
  badgeClass?: string
  /** Dim the badge: the number may be out of date, or is unknown. */
  badgeStale?: boolean
  /** Tooltip on the badge saying why it is dimmed. */
  badgeTitle?: string
  title: string
  /** Accessible name when the tab shows only an icon. */
  ariaLabel?: string
  children: ReactNode
}): React.JSX.Element {
  const text = badgeText(badge)
  return (
    <button
      onClick={onSelect}
      className={`relative flex h-6 items-center justify-center px-2 text-[11px] font-medium transition-colors ${
        selected
          ? 'text-[var(--color-text-primary)] bg-white/[0.08] shadow-[inset_0_-2px_0_var(--color-accent)]'
          : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'
      }`}
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag'
      }}
      title={title}
      aria-label={ariaLabel}
      aria-pressed={selected}
    >
      {children}
      {text !== null && (
        <span
          data-testid="page-tab-badge"
          data-stale={badgeStale ? 'true' : 'false'}
          title={badgeTitle}
          className={`absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] flex items-center justify-center text-[8px] font-bold text-[var(--color-on-accent)] rounded-full px-0.5 ${badgeClass ?? 'bg-[var(--color-accent)]'}${badgeStale ? ' opacity-50' : ''}`}
        >
          {text}
        </span>
      )}
    </button>
  )
}

export default function PageTabs(): React.JSX.Element {
  const page = usePageViewStore((s) => s.page)
  const setPage = usePageViewStore((s) => s.setPage)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const keybindings = useSettingsStore((s) => s.keybindings)
  const waitingCount = useFeedbackStore((s) => s.waitingCount)
  const waitingStale = useFeedbackStore((s) => s.waitingStale)
  const waitingUnsupported = useFeedbackStore((s) => s.waitingUnsupported)
  const ticketsBadge = ticketsBadgeProps({ waitingCount, waitingStale, waitingUnsupported })
  const projectsUnread = useProjectGroupsStore((s) => s.unreadGroupIds.size)
  const projects = useProjectsStore((s) => s.projects)
  // Membership only — reorder/color must not re-fan-out waiting-count
  // (that N+1 over E2E is the Settings-drag disconnect on large remotes).
  const projectIdsKey = projects.map((p) => p.id).join('\0')

  // Moved verbatim from the absorbed FeedbackTopBarButton: wire the
  // feedback:created/answered listeners once (idempotent) and (re)count
  // waiting items whenever the registered-projects set changes — the
  // count is a per-workspace list fan-out, so it must re-run when
  // workspaces appear/disappear. Only the MAIN window fires the desktop
  // notification (a second window would double-notify).
  useEffect(() => {
    let isMain = true
    try {
      isMain = getCurrentWindow().label === 'main'
    } catch {
      /* outside Tauri (tests) — default main */
    }
    initFeedbackEvents(isMain)
    void useFeedbackStore.getState().refreshWaitingCount()
  }, [projectIdsKey])

  // Project-group events (nav liveness + the unread badge). Idempotent,
  // same survival contract as initFeedbackEvents.
  useEffect(() => {
    initProjectGroupEvents()
  }, [])

  const select = (p: AppPage): void => {
    if (useSettingsStore.getState().settingsOpen) {
      useSettingsStore.getState().closeSettings()
    }
    if (p === 'wiki') {
      // Already on the wiki: keep the wiki shown (the picker switches it).
      if (usePageViewStore.getState().page !== 'wiki') openWikiForView()
      return
    }
    setPage(p)
  }

  return (
    <div className="no-drag flex items-center gap-0.5">
      <PageTab
        selected={settingsOpen}
        onSelect={() => {
          if (!useSettingsStore.getState().settingsOpen) {
            useSettingsStore.getState().openSettings()
          }
        }}
        title={withChord('Settings', 'Meta+,')}
        ariaLabel="Settings"
      >
        <SettingsIcon size={PAGE_TAB_ICON_PX} />
      </PageTab>
      {PAGE_TAB_ITEMS.map(({ page: p, Icon }) => {
        const badge =
          p === 'projects'
            ? { badge: projectsUnread }
            : p === 'feedback'
              ? {
                  badge: ticketsBadge.badge,
                  // Tickets waiting on you: the needs-you orange, not working.
                  badgeClass: 'bg-[var(--color-status-needs-you)]',
                  badgeStale: ticketsBadge.badgeStale,
                  badgeTitle: ticketsBadge.badgeTitle,
                }
              : {}
        return (
          <PageTab
            key={p}
            selected={!settingsOpen && page === p}
            onSelect={() => select(p)}
            title={pageTabTitle(p, keybindings, isMacPlatform())}
            ariaLabel={PAGE_TAB_LABELS[p].name}
            {...badge}
          >
            <Icon size={PAGE_TAB_ICON_PX} />
          </PageTab>
        )
      })}
    </div>
  )
}

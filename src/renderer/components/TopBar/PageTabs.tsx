// Projects V1 P4 (prd-projects-v1 §6.0) — the top bar's page switcher:
// ⚙ | Agents | Projects | Tickets. Replaces the standalone
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

function PageTab({
  selected,
  onSelect,
  badge,
  badgeClass,
  title,
  children,
}: {
  selected: boolean
  onSelect: () => void
  badge?: number
  badgeClass?: string
  title: string
  children: ReactNode
}): React.JSX.Element {
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
    >
      {children}
      {badge !== undefined && badge > 0 && (
        <span
          className={`absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] flex items-center justify-center text-[8px] font-bold text-[var(--color-on-accent)] rounded-full px-0.5 ${badgeClass ?? 'bg-[var(--color-accent)]'}`}
        >
          {badge > 99 ? '99+' : badge}
        </span>
      )}
    </button>
  )
}

export default function PageTabs(): React.JSX.Element {
  const page = usePageViewStore((s) => s.page)
  const setPage = usePageViewStore((s) => s.setPage)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const waitingCount = useFeedbackStore((s) => s.waitingCount)
  const projectsUnread = useProjectGroupsStore((s) => s.unreadGroupIds.size)
  const projects = useProjectsStore((s) => s.projects)

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
  }, [projects])

  // Project-group events (nav liveness + the unread badge). Idempotent,
  // same survival contract as initFeedbackEvents.
  useEffect(() => {
    initProjectGroupEvents()
  }, [])

  const select = (p: AppPage): void => {
    if (useSettingsStore.getState().settingsOpen) {
      useSettingsStore.getState().closeSettings()
    }
    setPage(p)
  }

  return (
    <div className="flex items-center gap-0.5">
      <PageTab
        selected={settingsOpen}
        onSelect={() => {
          if (!useSettingsStore.getState().settingsOpen) {
            useSettingsStore.getState().openSettings()
          }
        }}
        title="Settings (⌘,)"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
        </svg>
      </PageTab>
      <PageTab
        selected={!settingsOpen && page === 'agents'}
        onSelect={() => select('agents')}
        title="Agents — the workspace view"
      >
        Agents
      </PageTab>
      <PageTab
        selected={!settingsOpen && page === 'projects'}
        onSelect={() => select('projects')}
        badge={projectsUnread}
        title="Projects — grouped agents, shared dashboards"
      >
        Projects
      </PageTab>
      <PageTab
        selected={!settingsOpen && page === 'feedback'}
        onSelect={() => select('feedback')}
        badge={waitingCount}
        badgeClass="bg-[var(--color-status-working)]"
        title="Tickets — agents waiting on a human"
      >
        Tickets
      </PageTab>
    </div>
  )
}

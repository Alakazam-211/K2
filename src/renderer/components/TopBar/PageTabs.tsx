// Projects V1 P4 (prd-projects-v1 §6.0) — the top bar's 3-tab page
// switcher: Agents | Projects | Feedback. Replaces the standalone
// v0.40.26 FeedbackTopBarButton: its waiting-count badge moves onto the
// Feedback tab (same count, same show/hide semantics) and its event
// wiring (initFeedbackEvents + the badge re-count on projects changes)
// moves here unchanged. The Projects tab carries the unread-groups badge
// (§4.4). Rendered in the workspace TopBar AND in the overlay pages' top
// bars so the switcher (and the server dropdown beside it) is visible on
// all three pages; the selected tab is visually indicated.

import { useEffect } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { useFeedbackStore, initFeedbackEvents } from '@/stores/feedback'
import { useProjectGroupsStore, initProjectGroupEvents } from '@/stores/project-groups'
import { useProjectsStore } from '@/stores/projects'

function PageTab({
  label,
  selected,
  onSelect,
  badge,
  badgeClass,
  title,
}: {
  label: string
  selected: boolean
  onSelect: () => void
  badge?: number
  badgeClass?: string
  title: string
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
      {label}
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

  const select = (p: AppPage): void => setPage(p)

  return (
    <div className="flex items-center gap-0.5">
      <PageTab
        label="Agents"
        selected={page === 'agents'}
        onSelect={() => select('agents')}
        title="Agents — the workspace view"
      />
      <PageTab
        label="Projects"
        selected={page === 'projects'}
        onSelect={() => select('projects')}
        badge={projectsUnread}
        title="Projects — grouped agents, shared dashboards"
      />
      <PageTab
        label="Feedback"
        selected={page === 'feedback'}
        onSelect={() => select('feedback')}
        badge={waitingCount}
        badgeClass="bg-[var(--color-status-working)]"
        title="Feedback — agents waiting on you"
      />
    </div>
  )
}

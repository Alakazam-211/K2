// Projects V1 P4 (prd-projects-v1 §6.1, reshaped by §6.7) — the
// full-page Projects view.
//
// A top-level page selected from the §6.0 switcher; body idiom borrowed
// from FeedbackPage.tsx: a fixed-inset overlay with its own draggable
// top bar (which carries the ServerSwitcher + the 3-tab switcher, so
// the server dropdown stays visible here). LEFT NAV = ProjectNav
// (Pinned + list + member drawer), collapsible to an ICON RAIL via the
// top-bar toggle (§6.7.1 — the Agents page's sidebar-collapse idiom,
// persisted per client). The main area shows the selected project under
// DASHBOARDS-AS-TABS (§6.7.6: one tab per dashboard in `position`
// order + the Feedback tab — no header row, §6.7.2; project identity
// lives in the nav, settings via right-click / Settings → Projects).
//
// The project CHAT is a full-height right-hand side panel (§6.7.3),
// toggled from the top bar's right end (the Agents page's right-panel
// toggle idiom); the unread dot rides the toggle while closed. Esc
// never navigates away — it focuses the open dashboard's last-used
// terminal pane (§6.7.4, handled inside ProjectDashboard).
//
// P5 fills the dashboard tabs: ProjectDashboard renders the canonical
// layout blob as percent-width pane columns (kessel canonical-terminal
// attach + htmlDoc panes), with drag-and-drop, coalesced save-layout,
// and apply-on-open staleness (§6.2/§6.3). P7 fills the Feedback tab:
// ProjectFeedbackTab, the member-scoped feedback board (§6.6).

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import SettingsGearButton from '@/components/TopBar/SettingsGearButton'
import ProjectNav, { CreateProjectForm, ProjectNavRail } from './ProjectNav'
import ProjectDashboard from './ProjectDashboard'
import DashboardPresetsMenu from './DashboardPresetsMenu'
import ProjectChatPanel from './ProjectChatPanel'
import { ProjectFeedbackTab } from '@/components/Feedback/ProjectFeedbackTab'
import { fetchProjectGroupShow, type ProjectGroupShow } from './projects-api'
import {
  FEEDBACK_TAB,
  orderedDashboards,
  paneSwitchDigit,
  resolveActiveTab,
} from './project-tabs'
import { requestPaneShortcut } from './dashboard-dnd'

const TOPBAR_HEIGHT = 38

// ── Selected-project main area ────────────────────────────────────────────

function SelectedProjectView({
  show,
  error,
}: {
  show: ProjectGroupShow | null
  error: string | null
}): React.JSX.Element {
  // §6.7.6 — the active tab: a dashboard id or the Feedback sentinel.
  // Selection switches arrive as a fresh mount (the page keys this view
  // by group id), so tab state resets per project; resolveActiveTab
  // heals a stale id (deleted dashboard) to the first dashboard.
  const [tab, setTab] = useState<string | null>(null)

  const dashboards = useMemo(() => orderedDashboards(show?.dashboards ?? []), [show?.dashboards])
  const dashboardIds = useMemo(() => dashboards.map((d) => d.id), [dashboards])
  const dashboardIdsRef = useRef(dashboardIds)
  dashboardIdsRef.current = dashboardIds

  const activeTab = resolveActiveTab(tab, dashboardIds)

  // §6.7.3 — the chat side panel renders across ALL dashboard tabs
  // (per-project, never per-dashboard); open/closed is the persisted
  // store flag the top-bar toggle flips.
  const chatOpen = useProjectGroupsStore((s) => !s.chatCollapsed)

  // P5 — a member-row click (open/focus its canonical pane) lands on a
  // DASHBOARD tab (the first one when Feedback is active); the mounted
  // ProjectDashboard consumes the request itself.
  const paneRequestNonce = useProjectGroupsStore((s) => s.paneRequest?.nonce ?? null)
  useEffect(() => {
    if (paneRequestNonce === null) return
    setTab((cur) =>
      resolveActiveTab(cur, dashboardIdsRef.current) === FEEDBACK_TAB
        ? (dashboardIdsRef.current[0] ?? cur)
        : cur,
    )
  }, [paneRequestNonce])

  if (error) {
    return (
      <div className="flex-1 flex items-center justify-center px-8">
        <p className="text-[11px] text-red-400">Failed to load project: {error}</p>
      </div>
    )
  }
  if (!show) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <p className="text-sm text-[var(--color-text-muted)]">Loading project…</p>
      </div>
    )
  }

  const activeDashboard = dashboards.find((d) => d.id === activeTab) ?? null

  return (
    <div className="flex-1 flex min-h-0 min-w-0">
      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        {/* §6.7.2 — no project header row (identity lives in the nav;
            settings via right-click / Settings → Projects). §6.7.6 —
            dashboards ARE the tabs: one per dashboard + Feedback. */}
        <div className="px-4 pt-2 border-b border-[var(--color-border)] flex-shrink-0">
          {/* The scroll container holds only the tabs — the presets
              popover must not live under overflow-x-auto (clipping). */}
          <div className="flex items-center gap-1">
            <div className="flex items-center gap-1 overflow-x-auto flex-1 min-w-0">
              {dashboards.map((d) => (
                <button
                  key={d.id}
                  type="button"
                  onClick={() => setTab(d.id)}
                  className={`px-3 py-1.5 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer whitespace-nowrap ${
                    activeTab === d.id
                      ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                      : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
                  }`}
                  title={`Dashboard "${d.name}" — revision ${d.revision}`}
                >
                  {d.name}
                </button>
              ))}
              <button
                type="button"
                onClick={() => setTab(FEEDBACK_TAB)}
                className={`px-3 py-1.5 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer whitespace-nowrap ${
                  activeTab === FEEDBACK_TAB
                    ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                    : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
                }`}
              >
                Feedback
              </button>
            </div>
            {/* §6.8.4 — the layout-presets menu at the RIGHT end of the
                tab row; only meaningful over an open dashboard (the
                mounted ProjectDashboard applies the re-tile). Hidden
                for viewers (inside the component). */}
            {activeTab !== FEEDBACK_TAB && activeDashboard !== null && <DashboardPresetsMenu />}
          </div>
        </div>

        {activeTab === FEEDBACK_TAB ? (
          /* P7 (§6.6) — the member-scoped feedback board. Keyed upstream
             by group id (SelectedProjectView), so a project switch is a
             fresh list; `show` refetches only update the members prop. */
          <ProjectFeedbackTab members={show.members} />
        ) : activeDashboard ? (
          /* P5 — the pane grid (§6.2/§6.3). Keyed by dashboard id so a
             project/dashboard-tab switch is a fresh mount = apply-on-
             open; `show` refetches (same key) update props WITHOUT
             re-adopting the layout — the dashboard only marks itself
             stale. */
          <div className="flex-1 flex flex-col min-h-0 p-3">
            <ProjectDashboard
              key={activeDashboard.id}
              show={show}
              dashboard={activeDashboard}
            />
          </div>
        ) : (
          <div className="flex-1 flex items-center justify-center border border-dashed border-[var(--color-border)] m-3 text-center px-8">
            <p className="text-xs text-[var(--color-text-muted)]">
              This project has no dashboard yet.
            </p>
          </div>
        )}
      </div>

      {/* §6.7.3 — the per-PROJECT chat panel, full height on the right,
          across every tab. Not keyed by dashboard — `show` refetches
          update props without remounting, so the composer draft
          survives them (SelectedProjectView is keyed by group id
          upstream: a project switch is a fresh panel). */}
      {chatOpen && <ProjectChatPanel show={show} />}
    </div>
  )
}

// ── The page ──────────────────────────────────────────────────────────────

export default function ProjectsPage(): React.JSX.Element | null {
  const isOpen = usePageViewStore((s) => s.page === 'projects')
  const groups = useProjectGroupsStore((s) => s.groups)
  const selectedGroupId = useProjectGroupsStore((s) => s.selectedGroupId)
  const revision = useProjectGroupsStore((s) => s.revision)
  // §6.7.1 / §6.7.3 — the two per-client persisted toggles.
  const navCollapsed = useProjectGroupsStore((s) => s.navCollapsed)
  const chatCollapsed = useProjectGroupsStore((s) => s.chatCollapsed)
  const selectedUnread = useProjectGroupsStore((s) =>
    s.selectedGroupId !== null && s.unreadGroupIds.has(s.selectedGroupId),
  )

  const [show, setShow] = useState<ProjectGroupShow | null>(null)
  const [showError, setShowError] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)

  // Fetch the group list while open: on open and on every project-group
  // event (revision) — the store also coalesce-refetches on events, so
  // this mainly covers open-with-stale-state.
  useEffect(() => {
    if (!isOpen) return
    void useProjectGroupsStore.getState().fetchGroups()
  }, [isOpen, revision])

  // Fetch the selected group's show view (members + dashboards) on
  // selection change and on events.
  useEffect(() => {
    if (!isOpen || !selectedGroupId) {
      setShow(null)
      setShowError(null)
      return
    }
    let cancelled = false
    fetchProjectGroupShow(selectedGroupId)
      .then((data) => {
        if (cancelled) return
        setShow(data)
        setShowError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setShowError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
  }, [isOpen, selectedGroupId, revision])

  // §6.7.4 — NO page-level Esc-to-Agents here: on the Projects page Esc
  // focuses the open dashboard's last-used terminal pane instead (the
  // handler lives in ProjectDashboard; inputs stopPropagation their own
  // Esc). FeedbackPage keeps its own Esc behavior.

  // ⌘1…⌘9 — focus dashboard pane N (reading order). CAPTURE phase +
  // stopPropagation, because the Agents page's Cmd+digit workspace
  // switch (useTerminalShortcuts, mounted by the TerminalArea that
  // stays alive UNDER this overlay) is a window BUBBLE listener with no
  // page gate — swallowing the combo here is what keeps the two from
  // both firing. Attached only while the page is open, and swallowed
  // even on the Feedback tab / with no dashboard mounted (a hidden
  // workspace switch underneath would be worse than a no-op).
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      const num = paneSwitchDigit(e)
      if (num === null) return
      e.preventDefault()
      e.stopPropagation()
      requestPaneShortcut(num)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [isOpen])

  // Reset transient view state when the page closes (selection is
  // store-level and deliberately survives — reopening lands where the
  // user left off).
  useEffect(() => {
    if (!isOpen) setCreating(false)
  }, [isOpen])

  const selectedGroup = useMemo(
    () => groups?.find((g) => g.id === selectedGroupId) ?? null,
    [groups, selectedGroupId],
  )

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-[var(--color-bg)]">
      {/* Top bar — mirrors the workspace TopBar's left cluster: traffic-
          light spacer + wordmark + SERVER DROPDOWN + the page switcher
          (§6.0: both visible on every page), draggable. */}
      <div
        className="flex items-center border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 select-none flex-shrink-0"
        data-tauri-drag-region
        onMouseDown={(e) => {
          // The attribute alone only fires when the bar ITSELF is the click
          // target — the full-width flex child swallows most clicks, so the
          // bar wasn't draggable (TopBar.tsx has the same fallback).
          if ((e.target as HTMLElement).closest('button, input, select, .no-drag')) return
          void getCurrentWindow().startDragging()
        }}
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div className="flex items-center gap-2 flex-1">
          <div style={{ width: 70 }} />
          <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">
            K2
          </span>
          <ServerSwitcher />
          {/* Settings cog sits BETWEEN the server picker and the tabs
              (Rosson, 2026-07-06): K2 | Server | ⚙ | Tabs | … */}
          <SettingsGearButton />
          <div className="no-drag">
            <PageTabs />
          </div>
          {/* §6.7.1 — nav-collapse toggle, the TopBar primary-sidebar
              idiom (same icon/placement order: … | ⚙ | collapse). */}
          <button
            type="button"
            onClick={() => useProjectGroupsStore.getState().setNavCollapsed(!navCollapsed)}
            className="no-drag flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
            title="Toggle projects nav"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 14 14"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.3"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              {!navCollapsed ? (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="5" y1="2" x2="5" y2="12" />
                </>
              ) : (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="5" y1="2" x2="5" y2="12" strokeDasharray="1.5 1.5" />
                </>
              )}
            </svg>
          </button>
        </div>

        {/* Right: the chat-panel toggle (§6.7.3 — the TopBar right-panel
            idiom at the bar's right end); the unread dot rides it while
            the panel is closed. */}
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => useProjectGroupsStore.getState().setChatCollapsed(!chatCollapsed)}
            className="no-drag relative flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
            title="Toggle project chat"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 14 14"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.3"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              {!chatCollapsed ? (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="8.5" y1="2" x2="8.5" y2="12" />
                  <line x1="11" y1="5" x2="11" y2="9" strokeWidth="1.5" />
                </>
              ) : (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="8.5" y1="2" x2="8.5" y2="12" strokeDasharray="1.5 1.5" />
                </>
              )}
            </svg>
            {chatCollapsed && selectedUnread && (
              <span
                className="absolute top-0.5 right-0.5 w-1.5 h-1.5 rounded-full bg-[var(--color-accent)]"
                title="New project messages"
              />
            )}
          </button>
        </div>
      </div>

      {/* Body: left nav (expanded or icon rail, §6.7.1) + main area. */}
      <div className="flex-1 min-h-0 flex">
        {navCollapsed ? (
          <ProjectNavRail
            members={show?.id === selectedGroupId ? show?.members ?? null : null}
            pocWorkspaceId={show?.pocWorkspaceId ?? null}
          />
        ) : (
          <ProjectNav
            members={show?.id === selectedGroupId ? show?.members ?? null : null}
            pocWorkspaceId={show?.pocWorkspaceId ?? null}
          />
        )}

        <div className="flex-1 flex flex-col min-h-0 min-w-0">
          {groups === null ? (
            <div className="flex-1 flex items-center justify-center">
              <p className="text-sm text-[var(--color-text-muted)]">Loading projects…</p>
            </div>
          ) : groups.length === 0 ? (
            /* Empty state — no projects exist yet (create CTA). */
            <div className="flex-1 flex items-center justify-center px-8">
              <div className="w-full max-w-sm text-center">
                <p className="text-sm text-[var(--color-text-secondary)]">No projects yet</p>
                <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
                  A project groups agents into one effort — a shared dashboard, one chat, one
                  Point of Contact.
                </p>
                <div className="mt-4 text-left">
                  {creating ? (
                    <CreateProjectForm onDone={() => setCreating(false)} />
                  ) : (
                    <button
                      className="w-full flex items-center justify-center gap-2 px-3 py-2 text-xs bg-white/[0.04] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08] transition-colors"
                      onClick={() => setCreating(true)}
                    >
                      <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
                      </svg>
                      Create your first project
                    </button>
                  )}
                </div>
              </div>
            </div>
          ) : !selectedGroup ? (
            <div className="flex-1 flex items-center justify-center m-4 border border-dashed border-[var(--color-border)] text-xs text-[var(--color-text-muted)] text-center px-6">
              Select a project to open its dashboard.
            </div>
          ) : (
            /* Same staleness guard as the nav's member drawer: while a
               selection switch's `show` fetch is in flight, render the
               loading state — never the PREVIOUS project's data (P5:
               that would briefly mount the wrong project's panes). */
            <SelectedProjectView
              key={selectedGroup.id}
              show={show?.id === selectedGroup.id ? show : null}
              error={showError}
            />
          )}
        </div>
      </div>
    </div>
  )
}

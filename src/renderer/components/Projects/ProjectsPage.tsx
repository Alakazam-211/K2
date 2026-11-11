// Projects V1 P4 (prd-projects-v1 §6.1, reshaped by §6.7) — the
// full-page Projects view.
//
// A top-level page selected from the §6.0 switcher; body idiom borrowed
// from FeedbackPage.tsx: a fixed-inset overlay with its own draggable
// top bar (which carries the ServerSwitcher + the page switcher, so
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

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { usePageViewStore } from '@/stores/page-view'
import { useZenShown, zenShownNow } from '@/lib/zen/zen-view'
import { PageLiveContext } from '@/contexts/TabVisibilityContext'
import { titleBarDragOnMouseDown, titleBarOnContextMenu, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { useProjectGroupsStore } from '@/stores/project-groups'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import { topBarLeftClusterMinWidth, TRAFFIC_LIGHT_CLUSTER_GAP_PX } from '@/lib/desktop-chrome'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import K2MarkButton from '@/components/TopBar/K2MarkButton'
import TopBarUtilities from '@/components/TopBar/TopBarUtilities'
import AnimatedDrawer from '@/components/Layout/AnimatedDrawer'
import { DrawerToggleButton } from '@/components/Layout/DrawerTabBarToggle'
import { Surface } from '@/components/ui'
import ProjectNav, { ProjectNavRail } from './ProjectNav'
import ProjectDashboard from './ProjectDashboard'
import DashboardPresetsMenu from './DashboardPresetsMenu'
import ProjectChatPanel from './ProjectChatPanel'
import { NoDashboardState, ProjectListLoadState, ProjectLoadState, ProjectsEmptyState } from './ProjectStates'
import { ProjectFeedbackTab } from '@/components/Feedback/ProjectFeedbackTab'
import { fetchProjectGroupShow, partitionPinned, type ProjectGroupShow } from './projects-api'
import {
  FEEDBACK_TAB,
  orderedDashboards,
  paneSwitchDigit,
  resolveActiveTab,
} from './project-tabs'
import { requestPaneShortcut } from './dashboard-dnd'
import { primaryScope } from '@/kessel/server-scope'
import { useUnreadEntry } from '@/stores/unread'
import { UnreadBadge } from '@/components/common/UnreadBadge'
import { TOPBAR_HEIGHT_CSS } from '../../../shared/constants'

const TOPBAR_HEIGHT = TOPBAR_HEIGHT_CSS

// ── Selected-project main area ────────────────────────────────────────────

function SelectedProjectView({
  name,
  show,
  error,
  onReload,
}: {
  /** The project's name from the list (its state cards say which). */
  name: string
  show: ProjectGroupShow | null
  error: string | null
  /** Read the project again (Try again; after creating its dashboard). */
  onReload: () => void
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

  // No dashboard at all: the canvas says so (and offers one) until the
  // person picks Feedback; resolveActiveTab alone would land on Feedback,
  // so the no-dashboard state never showed.
  const activeTab: string | null =
    dashboardIds.length === 0 && tab !== FEEDBACK_TAB ? null : resolveActiveTab(tab, dashboardIds)

  // §6.7.3 — the chat side panel renders across ALL dashboard tabs
  // (per-project, never per-dashboard); open/closed is the persisted
  // store flag the top-bar toggle flips.
  const chatOpen = useProjectGroupsStore((s) => !s.chatCollapsed)
  // thread-read-v1: the selected project's chat state, per the daemon.
  const selectedId = useProjectGroupsStore((s) => s.selectedGroupId)
  const selectedUnread = useUnreadEntry('project', selectedId)

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

  if (error || !show) return <ProjectLoadState name={name} error={error} onRetry={onReload} />

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
            {/* Closed chat: its » / « toggle in its own slot at the row's
                far right (the drawer-toggle principle); the unread dot
                rides it. Open, the toggle sits in the chat header. */}
            {!chatOpen && (
              <div className="-mr-4 -mt-2 flex self-stretch" data-project-chat-toggle-slot="">
                <DrawerToggleButton
                  side="right"
                  placement="room"
                  name="project chat"
                  onToggle={() => useProjectGroupsStore.getState().setChatCollapsed(false)}
                >
                  {selectedUnread && (selectedUnread.unread > 0 || selectedUnread.waiting > 0) && (
                    <span className="absolute top-1 right-1 flex" data-project-chat-unread="">
                      <UnreadBadge
                        unread={selectedUnread.unread}
                        waiting={selectedUnread.waiting}
                        muted={selectedUnread.muted}
                        variant="dot"
                        label="project messages"
                      />
                    </span>
                  )}
                </DrawerToggleButton>
              </div>
            )}
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
          <NoDashboardState groupId={show.id} onCreated={onReload} />
        )}
      </div>

      {/* §6.7.3 — the per-PROJECT chat panel, full height on the right,
          across every tab. Not keyed by dashboard — `show` refetches
          update props without remounting, so the composer draft
          survives them (SelectedProjectView is keyed by group id
          upstream: a project switch is a fresh panel). */}
      <AnimatedDrawer open={chatOpen} side="right" bordered={false} clipAtRest={false}>
        <ProjectChatPanel show={show} />
      </AnimatedDrawer>
    </div>
  )
}

// ── The page ──────────────────────────────────────────────────────────────

export default function ProjectsPage(): React.JSX.Element | null {
  const isOpen = usePageViewStore((s) => s.page === 'projects')
  // prd-zen-gardens-v1 G4/G49: under Zen the page's terminals park.
  const zenShown = useZenShown()
  const groups = useProjectGroupsStore((s) => s.groups)
  const selectedGroupId = useProjectGroupsStore((s) => s.selectedGroupId)
  const revision = useProjectGroupsStore((s) => s.revision)
  // §6.7.1 / §6.7.3 — the two per-client persisted toggles.
  const navCollapsed = useProjectGroupsStore((s) => s.navCollapsed)

  const [show, setShow] = useState<ProjectGroupShow | null>(null)
  // The last show read's failure, for the project it was about (a switch
  // never shows the previous project's error).
  const [showError, setShowError] = useState<{ id: string; message: string } | null>(null)
  // Bumped by Try again / a created dashboard: read the project again.
  const [showNonce, setShowNonce] = useState(0)
  // Stay mounted after the first visit so a remote dashboard does not
  // re-pay spawn + grid handshake on every Agents ↔ Projects switch.
  const [everOpened, setEverOpened] = useState(false)

  // Fetch the group list while open: on open and on every project-group
  // event (revision) — the store also coalesce-refetches on events, so
  // this mainly covers open-with-stale-state.
  useEffect(() => {
    if (!isOpen) return
    void useProjectGroupsStore.getState().fetchGroups()
  }, [isOpen, revision])

  // Auto-select the first project (pinned section first, then list order)
  // when opening the Projects tab with nothing selected — avoids the empty
  // "Select a project…" shell on every cold open.
  useEffect(() => {
    if (!isOpen) return
    if (selectedGroupId) return
    if (!groups || groups.length === 0) return
    const { pinned, unpinned } = partitionPinned(groups)
    const first = pinned[0] ?? unpinned[0]
    if (first) useProjectGroupsStore.getState().selectGroup(first.id)
  }, [isOpen, groups, selectedGroupId])

  useEffect(() => {
    if (isOpen) setEverOpened(true)
  }, [isOpen])

  // Fetch the selected group's show view (members + dashboards) on
  // selection change and on events. Do not clear `show` when the page
  // hides — that would unmount the dashboard terminals we just kept.
  useEffect(() => {
    if (!isOpen || !selectedGroupId) return
    let cancelled = false
    fetchProjectGroupShow(primaryScope(), selectedGroupId)
      .then((data) => {
        if (cancelled) return
        setShow(data)
        setShowError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setShowError({ id: selectedGroupId, message: e instanceof Error ? e.message : String(e) })
      })
    return () => {
      cancelled = true
    }
  }, [isOpen, selectedGroupId, revision, showNonce])

  // Try again: clear the failure (the loading card shows) and read again.
  const reloadShow = useCallback((): void => {
    setShowError(null)
    setShowNonce((n) => n + 1)
  }, [])

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
      // prd-zen-gardens-v1 G26: under Zen, ⌘1–9 belongs to the Garden.
      if (zenShownNow()) return
      const num = paneSwitchDigit(e)
      if (num === null) return
      e.preventDefault()
      e.stopPropagation()
      requestPaneShortcut(num)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [isOpen])

  const selectedGroup = useMemo(
    () => groups?.find((g) => g.id === selectedGroupId) ?? null,
    [groups, selectedGroupId],
  )

  if (!everOpened) return null

  return (
    <PageLiveContext.Provider value={isOpen && !zenShown}>
    <div
      className="fixed inset-[var(--inset-window)] z-50 flex flex-col bg-[var(--color-bg)]"
      style={isOpen ? undefined : { display: 'none' }}
      aria-hidden={!isOpen}
    >
      {/* Top bar — mirrors the workspace TopBar's left cluster: traffic-
          light spacer + wordmark + the page switcher + SERVER DROPDOWN
          (§6.0), draggable. */}
      <Surface
        role2="surface" data-chrome="bar"
        bordered={false}
        className="flex items-center border-b border-[var(--color-border)] px-3 select-none flex-shrink-0"
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        onContextMenu={titleBarOnContextMenu}
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div
          className="flex items-center flex-1 [&>*]:shrink-0"
          style={{ minWidth: topBarLeftClusterMinWidth(), gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}
        >
          <DesktopChromeLeft />
          <K2MarkButton />
          <PageTabs />
          <ServerSwitcher />
        </div>

        {/* Right: utilities + window controls. The nav and chat toggles
            moved off the top bar (0.46.0): the nav's » / « sits at the
            bottom of the nav / rail, the chat's in the dashboard tab row
            (closed) or the chat header (open). */}
        <DesktopChromeRight>
          <TopBarUtilities />
        </DesktopChromeRight>
      </Surface>

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
            /* Loading, or the list read failed (Try again). */
            <ProjectListLoadState />
          ) : groups.length === 0 ? (
            /* No projects yet: what a project is, and New project. */
            <ProjectsEmptyState />
          ) : !selectedGroup ? (
            <div className="flex-1 flex items-center justify-center px-6">
              <p className="type-hint text-[var(--color-text-secondary)]">Pick a project on the left to open it.</p>
            </div>
          ) : (
            /* Same staleness guard as the nav's member drawer: while a
               selection switch's `show` fetch is in flight, render the
               loading state — never the PREVIOUS project's data (P5:
               that would briefly mount the wrong project's panes). */
            <SelectedProjectView
              key={selectedGroup.id}
              name={selectedGroup.name}
              show={show?.id === selectedGroup.id ? show : null}
              error={showError?.id === selectedGroup.id ? showError.message : null}
              onReload={reloadShow}
            />
          )}
        </div>
      </div>
      <div
        data-toast-host="projects"
        className="pointer-events-none overflow-hidden"
        style={{ position: 'absolute', top: TOPBAR_HEIGHT, right: 0, bottom: 0, left: 0, zIndex: 40 }}
      />
    </div>
    </PageLiveContext.Provider>
  )
}

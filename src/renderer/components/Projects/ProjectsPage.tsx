// Projects V1 P4 (prd-projects-v1 §6.1, reshaped by §6.7) — the
// full-page Projects view.
//
// A top-level page selected from the §6.0 switcher; body idiom borrowed
// from FeedbackPage.tsx: a fixed-inset overlay with its own draggable
// top bar (which carries the ServerSwitcher + the page switcher, so
// the server dropdown stays visible here). LEFT NAV = ProjectNav: the
// project list (Pinned, All projects), then the selected project's agents
// and their files, in the Home and Agents navs' panel (NavTile);
// collapsible to an ICON RAIL from the « in its top row (» at the top of
// the rail). The main area shows the selected project under its
// PROJECT BAR (ProjectBar.tsx, projects-reorg-draft §3.2): identity, the
// dashboards as tabs + Feedback, and Layout, Settings and Chat.
//
// The project CHAT is a full-height right-hand side panel (§6.7.3),
// opened from the bar's Chat button (with the unread count) and closed
// from the × in the chat's own header. Esc never navigates away — it
// focuses the open dashboard's last-used terminal pane (§6.7.4, handled
// inside ProjectDashboard).
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
import { Surface } from '@/components/ui'
import { NavTile } from '@/components/Layout/NavTile'
import ProjectNav, { PROJECT_NAV_WIDTH, ProjectNavRail } from './ProjectNav'
import ProjectDashboard from './ProjectDashboard'
import ProjectBar, { useViewerRole } from './ProjectBar'
import { projectSettingsCaps } from './project-settings'
import ProjectChatPanel from './ProjectChatPanel'
import { NoDashboardState, ProjectListLoadState, ProjectLoadState, ProjectsEmptyState } from './ProjectStates'
import ProjectSettingsSheet from './ProjectSettingsSheet'
import { ProjectFeedbackTab } from '@/components/Feedback/ProjectFeedbackTab'
import { fetchProjectGroupShow, partitionPinned, pathIsMember, type ProjectGroupShow } from './projects-api'
import {
  FEEDBACK_TAB,
  orderedDashboards,
  paneSwitchDigit,
  resolveActiveTab,
} from './project-tabs'
import { requestPaneShortcut } from './dashboard-dnd'
import { primaryScope } from '@/kessel/server-scope'
import { onSessionAddedApp, onSessionRemovedApp } from '@/stores/session-events'
import { TOPBAR_HEIGHT_CSS } from '../../../shared/constants'

const TOPBAR_HEIGHT = TOPBAR_HEIGHT_CSS

// ── Selected-project main area ────────────────────────────────────────────

type TabUpdate = string | null | ((cur: string | null) => string | null)

function SelectedProjectView({
  name,
  show,
  error,
  tab,
  setTab,
  onReload,
  retrying = false,
}: {
  /** The project's name from the list (its state cards say which). */
  name: string
  show: ProjectGroupShow | null
  error: string | null
  /** Try again in flight (its button busy on the failure card). */
  retrying?: boolean
  /** §6.7.6 — the chosen tab (a dashboard id or the Feedback sentinel),
   *  held by the page per project so the nav knows the dashboard on
   *  screen; resolveActiveTab heals a stale id to the first dashboard. */
  tab: string | null
  setTab: (next: TabUpdate) => void
  /** Read the project again (Try again; after creating its dashboard or
   *  any change made from its bar or the nav). */
  onReload: () => void
}): React.JSX.Element {

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
  // store flag the bar's Chat button and the chat's own × flip.
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

  if (error || !show) return <ProjectLoadState name={name} error={error} onRetry={onReload} retrying={retrying} />

  const activeDashboard = dashboards.find((d) => d.id === activeTab) ?? null

  return (
    <div className="flex-1 flex min-h-0 min-w-0">
      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        {/* The project bar (projects-reorg-draft §3.2): which project
            this is, its dashboards as tabs + Feedback, and its actions. */}
        <ProjectBar
          show={show}
          dashboards={dashboards}
          activeTab={activeTab}
          onTab={setTab}
          onChanged={onReload}
        />

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
  // Try again in flight: the failure stays up with its button busy.
  const [showRetrying, setShowRetrying] = useState(false)
  // Bumped when one of the project's agents starts or ends a session: the
  // nav's awake/asleep comes from `show` (a snapshot), so read it again.
  const [sessionsNonce, setSessionsNonce] = useState(0)
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
        setShowRetrying(false)
      })
      .catch((e) => {
        if (cancelled) return
        setShowError({ id: selectedGroupId, message: e instanceof Error ? e.message : String(e) })
        setShowRetrying(false)
      })
    return () => {
      cancelled = true
    }
  }, [isOpen, selectedGroupId, revision, showNonce, sessionsNonce])

  // An agent of this project woke or went to sleep: read `show` again
  // (one call, coalesced), never one lookup per agent. The app-level
  // session events cover every workspace.
  const membersRef = useRef<ProjectGroupShow['members']>([])
  membersRef.current = show?.id === selectedGroupId ? show.members : []
  useEffect(() => {
    if (!isOpen || !selectedGroupId) return
    let timer: ReturnType<typeof setTimeout> | null = null
    const onSession = (e: { workspace_path: string }): void => {
      if (!pathIsMember(membersRef.current, e.workspace_path)) return
      if (timer !== null) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        setSessionsNonce((n) => n + 1)
      }, 400)
    }
    const offAdded = onSessionAddedApp(primaryScope(), onSession)
    const offRemoved = onSessionRemovedApp(primaryScope(), onSession)
    return () => {
      if (timer !== null) clearTimeout(timer)
      offAdded()
      offRemoved()
    }
  }, [isOpen, selectedGroupId])

  // Read the project again (Try again, a created dashboard). A failure
  // stays up meanwhile, its Try again busy ("Trying again…").
  const reloadShow = useCallback((): void => {
    setShowRetrying(true)
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

  // The selected project's show, never the PREVIOUS project's while a
  // switch's fetch is in flight.
  const selectedShow = show && show.id === selectedGroupId ? show : null
  // The login's role: who may add an agent from the nav (any login today).
  const role = useViewerRole()
  const caps = projectSettingsCaps(role)
  const canAddAgents = caps.canEdit
  // Each project's chosen tab, kept here so the nav knows the dashboard on
  // screen (its files say "On this dashboard"). On Feedback, the first
  // dashboard: where a "Show on this dashboard" lands.
  const [tabByGroup, setTabByGroup] = useState<Record<string, string | null>>({})
  const navDashboard = useMemo(() => {
    const ds = orderedDashboards(selectedShow?.dashboards ?? [])
    const active = resolveActiveTab(selectedGroupId ? (tabByGroup[selectedGroupId] ?? null) : null, ds.map((d) => d.id))
    return ds.find((d) => d.id === active) ?? ds[0] ?? null
  }, [selectedShow, selectedGroupId, tabByGroup])

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
            moved off the top bar (0.46.0): the nav's « sits in its top
            row and the rail's » at the rail's top, the chat's in the dashboard tab row
            (closed) or the chat header (open). */}
        <DesktopChromeRight>
          <TopBarUtilities />
        </DesktopChromeRight>
      </Surface>

      {/* Body: left nav (expanded or icon rail, §6.7.1) + main area, the
          pane gap between them as on Home and Agents (Layout.tsx). */}
      <div className="flex-1 min-h-0 flex gap-[var(--gap-pane)]">
        {/* The nav: the project list, then the selected project's agents
            and their files (ProjectAgentsSection), in the Home and Agents
            navs' panel (NavTile). The rail is a tile of its own. */}
        {navCollapsed ? (
          <ProjectNavRail show={selectedShow} canAddAgents={canAddAgents} onChanged={reloadShow} />
        ) : (
          <NavTile style={{ width: PROJECT_NAV_WIDTH }}>
            <ProjectNav
              show={selectedShow}
              dashboard={navDashboard}
              canAddAgents={canAddAgents}
              canSetAgentViews={caps.canEditLooks}
              onChanged={reloadShow}
            />
          </NavTile>
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
              retrying={showRetrying}
              tab={tabByGroup[selectedGroup.id] ?? null}
              setTab={(next) =>
                setTabByGroup((all) => {
                  const cur = all[selectedGroup.id] ?? null
                  return { ...all, [selectedGroup.id]: typeof next === 'function' ? next(cur) : next }
                })
              }
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
      {/* A project's settings over the page (right-click → Project settings). */}
      <ProjectSettingsSheet />
    </div>
    </PageLiveContext.Provider>
  )
}

// The Tickets page — the full-page agent→human ask queue
// (prd-agent-feedback-notifications §6 F2; 0.43.2 quick redesign).
//
// A fixed inset view over the app, opened from the top-bar Tickets tab
// (useFeedbackStore), with its own draggable top bar; Esc closes it (the
// board clears an open ticket first). The list reads
// `/cli/feedback/list-all?all=1` (feedback-api) and stays live via the
// store's `revision` (bumped by the feedback:* listeners) — no polling.
//
// The board itself (narrow list + brief-first detail + chat rail) is the
// shared TicketBoard; this page adds the workspace/project filter (§6.6)
// beside the board's search.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { useFeedbackStore } from '@/stores/feedback'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import { topBarLeftClusterMinWidth, TRAFFIC_LIGHT_CLUSTER_GAP_PX } from '@/lib/desktop-chrome'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import K2MarkButton from '@/components/TopBar/K2MarkButton'
import TopBarUtilities from '@/components/TopBar/TopBarUtilities'
import { Surface } from '@/components/ui'
import { fetchAllFeedback, isUnlinked, type FeedbackListRow } from './feedback-api'
import {
  parseProjectFilter,
  rowsForWorkspaceFilter,
  UNLINKED_FILTER_VALUE,
  WorkspaceFilterDropdown,
} from './WorkspaceFilterDropdown'
import { fetchProjectGroupShow } from '@/components/Projects/projects-api'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { primaryScope } from '@/kessel/server-scope'
import { TicketBoard } from './TicketBoard'

// The card + its pieces moved to TicketCard.tsx; re-exported for callers.
export { FeedbackCard, SectionHeader, cardAssigneeNames } from './TicketCard'

const TOPBAR_HEIGHT = 38

export default function FeedbackPage(): React.JSX.Element | null {
  const isOpen = useFeedbackStore((s) => s.isOpen)
  const close = useFeedbackStore((s) => s.close)
  const revision = useFeedbackStore((s) => s.revision)
  const projects = useProjectsStore((s) => s.projects)

  const [rows, setRows] = useState<FeedbackListRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  // Workspace filter — a workspace id, `project:<groupId>` (§6.6), or 'all'.
  const [workspaceFilter, setWorkspaceFilter] = useState<string>('all')
  const [projectMemberIds, setProjectMemberIds] = useState<Set<string> | null>(null)

  const loadList = useCallback(async (): Promise<void> => {
    try {
      const data = await fetchAllFeedback(
        useProjectsStore.getState().projects.map((p) => ({ id: p.id, name: p.name, path: p.path })),
      )
      setRows(data)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setRows((prev) => prev ?? [])
    }
  }, [])

  // Fetch while open: on open, on every feedback event (revision), and
  // when the registered-projects set changes.
  useEffect(() => {
    if (!isOpen) return
    void loadList()
  }, [isOpen, revision, projects, loadList])

  // Resolve the selected project filter's membership (§6.6), refreshed on
  // project-group events. A failed resolve logs and leaves the set null.
  const pgRevision = useProjectGroupsStore((s) => s.revision)
  const projectFilterId = parseProjectFilter(workspaceFilter)
  useEffect(() => {
    if (!isOpen || projectFilterId === null) {
      setProjectMemberIds(null)
      return
    }
    let cancelled = false
    fetchProjectGroupShow(primaryScope(), projectFilterId)
      .then((show) => {
        if (cancelled) return
        setProjectMemberIds(new Set(show.members.map((m) => m.workspaceId)))
      })
      .catch((err) => {
        if (cancelled) return
        console.warn('[feedback] project-filter membership resolve failed:', err)
        setProjectMemberIds(null)
      })
    return () => {
      cancelled = true
    }
  }, [isOpen, projectFilterId, pgRevision])

  // Esc closes the page. The board's capture-phase Esc clears an open
  // ticket first (and stops the event), so this only sees Esc with none.
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [isOpen, close])

  // Reset transient view state when the page closes.
  useEffect(() => {
    if (!isOpen) {
      setRows(null)
      setError(null)
    }
  }, [isOpen])

  const scoped = useMemo(
    () => (rows ? rowsForWorkspaceFilter(rows, workspaceFilter, projectMemberIds) : null),
    [rows, workspaceFilter, projectMemberIds],
  )

  // "Unlinked workspace" is offered only while such rows exist; once the
  // last one is gone, an active unlinked filter falls back to All.
  const hasUnlinked = useMemo(() => (rows ?? []).some(isUnlinked), [rows])
  useEffect(() => {
    if (rows !== null && workspaceFilter === UNLINKED_FILTER_VALUE && !hasUnlinked) {
      setWorkspaceFilter('all')
    }
  }, [rows, workspaceFilter, hasUnlinked])

  const onMutated = useCallback((): void => {
    void loadList()
    void useFeedbackStore.getState().refreshWaitingCount()
  }, [loadList])

  if (!isOpen) return null

  return (
    <div className="fixed inset-[var(--inset-window)] z-50 flex flex-col bg-[var(--color-bg)]">
      {/* Top bar — traffic-light spacer + wordmark, draggable. */}
      <Surface
        role2="surface"
        bordered={false}
        className="flex items-center border-b border-[var(--color-border)] px-3 select-none flex-shrink-0"
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div
          className="flex items-center flex-1 [&>*]:shrink-0"
          style={{ minWidth: topBarLeftClusterMinWidth(), gap: TRAFFIC_LIGHT_CLUSTER_GAP_PX }}
        >
          <DesktopChromeLeft />
          <K2MarkButton />
          <ServerSwitcher />
          <PageTabs />
        </div>

        <DesktopChromeRight>
          <TopBarUtilities>
            <button
              type="button"
              onClick={close}
              className="no-drag flex h-6 w-6 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer"
              title="Close (Esc)"
            >
              <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
                <line x1="2" y1="2" x2="10" y2="10" />
                <line x1="10" y1="2" x2="2" y2="10" />
              </svg>
            </button>
          </TopBarUtilities>
        </DesktopChromeRight>
      </Surface>

      <TicketBoard
        rows={scoped}
        allRows={rows}
        error={error}
        revision={revision}
        onMutated={onMutated}
        extraFilter={
          <WorkspaceFilterDropdown
            projects={projects}
            value={workspaceFilter}
            onChange={setWorkspaceFilter}
            showUnlinked={hasUnlinked}
          />
        }
      />
      <div
        data-toast-host="feedback"
        className="pointer-events-none overflow-hidden"
        style={{ position: 'absolute', top: TOPBAR_HEIGHT, right: 0, bottom: 0, left: 0, zIndex: 40 }}
      />
    </div>
  )
}

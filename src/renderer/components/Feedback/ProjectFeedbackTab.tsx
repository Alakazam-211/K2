// Projects V1 P7 (prd-projects-v1 §6.6) — the Projects page's Feedback
// tab: the PROJECT-SCOPED tickets board.
//
// Recycled machinery only: the list is the same fetch as the Tickets page
// (fetchAllFeedback) kept to the selected project's MEMBER workspaces, and
// the board is the shared TicketBoard (narrow list + brief-first detail +
// chat rail). Live updates ride the SAME feedback store `revision` the
// Tickets page refetches on. No workspace filter here: the project IS the
// filter.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useFeedbackStore } from '@/stores/feedback'
import type { ProjectGroupMemberInfo } from '@/components/Projects/projects-api'
import { fetchAllFeedback, type FeedbackListRow, type FeedbackProjectRef } from './feedback-api'
import { TicketBoard } from './TicketBoard'
import { ProjectStateCard, StateCanvas, StateRetry } from '@/components/Projects/StateCard'

/** The empty tab's icon (a ticket is an agent's question for a person). */
function AskIcon(): React.JSX.Element {
  return (
    <svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} strokeLinecap="round" strokeLinejoin="round">
      <path d="M4 5.5h16v10.5H9.5L5 19.5V16H4z" />
      <path d="M10 9.2a2 2 0 1 1 2.6 1.9c-.4.15-.6.5-.6.9v.3M12 13.9v.1" />
    </svg>
  )
}

/** Pure scoping helper (unit-tested): the fan-out refs for a project's
 *  member list. Members whose workspace has been unregistered since
 *  they were added carry null name/path — they have no list route to
 *  hit, so they are skipped. */
export function memberFeedbackRefs(members: ProjectGroupMemberInfo[]): FeedbackProjectRef[] {
  const refs: FeedbackProjectRef[] = []
  for (const m of members) {
    if (m.name === null || m.path === null) continue
    refs.push({ id: m.workspaceId, name: m.name, path: m.path })
  }
  return refs
}

/** Keep only tickets filed by the project's member workspaces. The host
 *  `list-all` read returns every ticket on the server, so the tab must
 *  narrow it itself (the per-workspace fan-out already is). */
export function rowsForMembers(
  rows: FeedbackListRow[],
  refs: readonly FeedbackProjectRef[],
): FeedbackListRow[] {
  const ids = new Set(refs.map((r) => r.id))
  return rows.filter((r) => ids.has(r.projectId))
}

export function ProjectFeedbackTab({
  members,
}: {
  /** The selected project's member rows (the page owns the `show` fetch). */
  members: ProjectGroupMemberInfo[]
}): React.JSX.Element {
  const revision = useFeedbackStore((s) => s.revision)

  const [rows, setRows] = useState<FeedbackListRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [retrying, setRetrying] = useState(false)

  const refs = useMemo(() => memberFeedbackRefs(members), [members])
  // Stable membership key: `show` refetches rebuild the members array on
  // every project-group event (incl. chat messages) — only an ACTUAL
  // membership change should re-run the fetch.
  const refsKey = useMemo(() => refs.map((r) => `${r.id} ${r.path}`).join('\n'), [refs])
  const refsRef = useRef(refs)
  refsRef.current = refs

  const loadList = useCallback(async (): Promise<void> => {
    try {
      const data = await fetchAllFeedback(refsRef.current)
      setRows(rowsForMembers(data, refsRef.current))
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setRows((prev) => prev ?? [])
    }
  }, [])

  useEffect(() => {
    // refsKey stands in for the refs (see above).
    void loadList()
  }, [revision, refsKey, loadList])

  const retry = useCallback((): void => {
    setRetrying(true)
    void loadList().finally(() => setRetrying(false))
  }, [loadList])

  const onMutated = useCallback((): void => {
    void loadList()
    void useFeedbackStore.getState().refreshWaitingCount()
  }, [loadList])

  // Nothing loaded and the read failed: one state card in place of the
  // board (a refetch failing under loaded tickets keeps the board's
  // line). Try again keeps the card up, its button busy.
  if (error && (rows === null || rows.length === 0)) {
    return (
      <StateCanvas>
        <ProjectStateCard
          kind="error"
          title="Couldn’t load this project’s tickets"
          detail={error}
          testId="project-feedback-error"
          action={<StateRetry onClick={retry} busy={retrying} />}
        >
          K2 couldn’t get them from the server. Your agents are fine.
        </ProjectStateCard>
      </StateCanvas>
    )
  }

  // Nothing yet: one card in place of the board (no list to search, no
  // detail pane asking for a pick).
  if (rows !== null && rows.length === 0) {
    return (
      <StateCanvas>
        <ProjectStateCard kind="empty" icon={<AskIcon />} title="No tickets yet" testId="project-feedback-empty">
          When an agent in this project files a ticket for you, it shows up here, as on the Tickets page.
        </ProjectStateCard>
      </StateCanvas>
    )
  }

  return (
    <TicketBoard
      rows={rows}
      error={error}
      revision={revision}
      onMutated={onMutated}
      onRetry={loadList}
    />
  )
}

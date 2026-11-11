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
import { SettingsLoadState } from '@/components/Settings/controls/SettingControls'
import { TryAgain } from '@/components/Projects/TileState'

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
  // board (a refetch failing under loaded tickets keeps the board's line).
  if (error && !retrying && (rows === null || rows.length === 0)) {
    return (
      <div className="flex-1 min-h-0 flex items-center justify-center p-6">
        <div className="w-full max-w-md">
          <SettingsLoadState
            kind="error"
            title="Couldn’t load this project’s tickets"
            detail={error}
            testId="project-feedback-error"
            action={<TryAgain onClick={retry} />}
          >
            K2 couldn’t read the tickets from this server.
          </SettingsLoadState>
        </div>
      </div>
    )
  }

  return (
    <TicketBoard
      rows={rows}
      error={error}
      revision={revision}
      onMutated={onMutated}
      emptyHint="When an agent in this project asks you something, it shows up here."
    />
  )
}

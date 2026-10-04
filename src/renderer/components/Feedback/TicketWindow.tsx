// Tickets "Open in window": a window that shows one ticket — the same
// detail as the board (header, brief, action bar, chat rail) under a
// draggable title bar. Opened by `window_open_ticket` (menu.rs).

import React, { useCallback, useEffect, useState } from 'react'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { getDesktopChrome } from '@/lib/desktop-chrome'
import { useFeedbackStore } from '@/stores/feedback'
import { fetchFeedbackShow, type FeedbackListRow } from './feedback-api'
import { briefOverlayStoplightInset } from './BriefFrame'
import { FeedbackItemView } from './FeedbackItemView'

const TOPBAR_HEIGHT = 38

export function TicketWindow({ ticketId }: { ticketId: string }): React.JSX.Element {
  const revision = useFeedbackStore((s) => s.revision)
  const [row, setRow] = useState<FeedbackListRow | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))

  const load = useCallback(async () => {
    try {
      const show = await fetchFeedbackShow(ticketId)
      setRow({
        ...show,
        assignees: show.assignees ?? [],
        projectPath: show.projectPath,
        projectName: show.workspace,
        linked: show.workspace !== null || show.projectPath !== null,
      })
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [ticketId])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [])

  const stoplightInset = briefOverlayStoplightInset(getDesktopChrome())

  return (
    <div className="fixed inset-[var(--inset-window)] flex flex-col bg-[var(--color-bg)]" data-testid="ticket-window">
      <div
        className="flex items-center gap-3 px-3 border-b border-[var(--color-border)] flex-shrink-0 select-none"
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
      >
        {stoplightInset > 0 && <div aria-hidden className="k2-stoplight-spacer-brief" />}
        <span className="text-[11px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
          Ticket
        </span>
      </div>
      {error ? (
        <div className="flex-1 px-4 py-3 text-[11px] text-[var(--color-status-error-soft)] selectable-copy">
          Failed to load ticket: {error}
        </div>
      ) : row ? (
        <FeedbackItemView
          id={row.id}
          listRow={row}
          nowSec={nowSec}
          revision={revision}
          onMutated={() => void load()}
          inOwnWindow
        />
      ) : (
        <div className="flex-1 flex items-center justify-center text-xs text-[var(--color-text-muted)]">
          Loading ticket…
        </div>
      )}
    </div>
  )
}

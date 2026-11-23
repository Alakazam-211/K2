// The one click on an owner-notice ticket (plan-getting-started-settings-v1):
// a ticket on the owner notices' workspace is read-only in the app (no
// discussion rail), so its options stay hidden, unless the daemon says who
// may pick them (`optionRoles`). Then the options the viewer's role may pick
// show here, under the report, as the rail's quick answers do. A pick is the
// ticket comment with `optionPick` (`feedback/comment`): the daemon records
// the answer and acts on it with the picker as the actor (a Member's ask:
// the setup list goes on). Mounted only for a ticket that carries
// `optionRoles`.

import { useCallback, useEffect, useState } from 'react'
import { ThreadChatChip, ThreadChatChips, ThreadChatNote } from '@/components/Thread/ThreadChat'
import { commentFeedback, noticeOptionsFor, ticketViewerRole, type FeedbackShow, type TicketViewerRole } from './feedback-api'
import { fetchTicketWhoami } from './useTicketMe'

/** The viewer's role from `auth/whoami`; null while it loads or if it fails. */
export function useTicketViewerRole(): TicketViewerRole | null {
  const [role, setRole] = useState<TicketViewerRole | null>(null)
  useEffect(() => {
    let live = true
    fetchTicketWhoami().then(
      (w) => {
        if (live) setRole(ticketViewerRole(w))
      },
      () => {
        if (live) setRole(null)
      },
    )
    return () => {
      live = false
    }
  }, [])
  return role
}

/** The picks themselves, for a known role (tests drive this one). */
export function NoticeQuickAnswersFor({
  item,
  role,
  onChanged,
}: {
  item: FeedbackShow
  role: TicketViewerRole | null
  onChanged: () => void
}): React.JSX.Element | null {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const options = noticeOptionsFor(item, role)
  const pick = useCallback(
    async (answer: string): Promise<void> => {
      if (busy) return
      setBusy(true)
      setError(null)
      try {
        await commentFeedback(item.id, answer, { optionPick: true })
        onChanged()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [busy, item.id, onChanged],
  )
  if (options.length === 0 || item.status === 'resolved' || item.status === 'dismissed') return null
  return (
    <div className="px-4 pb-3" data-testid="ticket-notice-answers">
      {error && <ThreadChatNote error>{error}</ThreadChatNote>}
      <ThreadChatChips testId="ticket-quick-answers">
        {options.map((opt) => (
          <ThreadChatChip
            key={opt.answer}
            testId="ticket-quick-answer"
            on={item.answer === opt.answer}
            disabled={busy}
            title={opt.detail === opt.label ? 'Send this as the answer' : opt.detail}
            onClick={() => void pick(opt.answer)}
          >
            {opt.label}
          </ThreadChatChip>
        ))}
      </ThreadChatChips>
    </div>
  )
}

export function NoticeQuickAnswers({ item, onChanged }: { item: FeedbackShow; onChanged: () => void }): React.JSX.Element | null {
  const role = useTicketViewerRole()
  return <NoticeQuickAnswersFor item={item} role={role} onChanged={onChanged} />
}

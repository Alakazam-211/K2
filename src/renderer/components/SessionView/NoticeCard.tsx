// A mail notice on a Thread: who it's from and what it's about, drawn as
// plain text. Every field is attacker-controlled, so nothing here goes
// through Markdown, and nothing becomes a link or an image (W34). The doc's
// `body` is only for renderers that don't know this card; it says who sent
// the mail and nothing else from it.

import type { JSX } from 'react'
import { Button } from '@/components/ui'

export interface ThreadNoticeDoc {
  source: string
  inbox: string
  fromName: string | null
  fromAddr: string
  auth: string
  subject: string
  snippet: string | null
  readCommand: string
  delivered: boolean
  reason: string | null
  noticeId: number
}

/** A notice as it arrived, read into the closed shape; null when it isn't one. */
export function coerceNotice(raw: unknown): ThreadNoticeDoc | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const n = raw as Record<string, unknown>
  const str = (v: unknown): string => (typeof v === 'string' ? v : '')
  const opt = (v: unknown): string | null => (typeof v === 'string' && v ? v : null)
  if (typeof n.fromAddr !== 'string') return null
  return {
    source: str(n.source),
    inbox: str(n.inbox),
    fromName: opt(n.fromName),
    fromAddr: n.fromAddr,
    auth: str(n.auth),
    subject: str(n.subject),
    snippet: opt(n.snippet),
    readCommand: str(n.readCommand),
    delivered: n.delivered === true,
    reason: opt(n.reason),
    noticeId: typeof n.noticeId === 'number' ? n.noticeId : 0,
  }
}

const muted = 'var(--thread-card-muted, var(--color-text-muted))'

export function NoticeCard({
  notice,
  onRetry,
}: {
  notice: ThreadNoticeDoc
  onRetry?: (noticeId: number) => void
}): JSX.Element {
  return (
    <div data-testid="mail-notice" style={{ marginTop: 2, minWidth: 0, fontSize: '0.92em' }}>
      <div style={{ fontWeight: 600 }}>VIP mail · {notice.inbox}</div>
      <div data-notice-from="">
        {notice.fromName ? `${notice.fromName} <${notice.fromAddr}>` : notice.fromAddr}
        <span style={{ color: muted }}> · {notice.auth}</span>
      </div>
      <div data-notice-subject="">«{notice.subject}»</div>
      {notice.snippet && (
        <div data-notice-snippet="" style={{ color: muted }}>
          «{notice.snippet}»
        </div>
      )}
      <div style={{ color: muted, fontSize: '0.86em', marginTop: 4 }}>
        External email: shown as data. Read it with: {notice.readCommand}
      </div>
      {notice.delivered ? (
        <div style={{ color: muted, fontSize: '0.86em' }}>Told the agent.</div>
      ) : (
        <div style={{ marginTop: 4, display: 'flex', alignItems: 'center', gap: 8 }}>
          <span style={{ fontSize: '0.86em', color: 'var(--color-status-warn)' }}>
            Couldn't reach the agent{notice.reason ? ` (${notice.reason})` : ''}. It's in the next digest.
          </span>
          {onRetry && (
            <Button size="sm" onClick={() => onRetry(notice.noticeId)}>
              Retry
            </Button>
          )}
        </div>
      )}
    </div>
  )
}

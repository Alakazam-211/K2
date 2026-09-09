// Chat about this — inject a kind-prefixed inbox stamp into pinned Chat.
// Dialog lives outside `data-testid="inbox-browser"` so I16 `\bSend\b` holds.

import { useEffect, useState } from 'react'
import { DialogFrame, DialogScrim } from '@/components/ui'
import { daemonCliPost } from '@/lib/daemon-cli'
import { activateProject } from '@/stores/projects'
import {
  mapMsgResponseToStatus,
  shouldSendOnKey,
  type ComposeStatus,
  type MsgResponse,
} from '@/components/Terminal/terminalCompose'
import {
  chatAboutStampLine,
  formatChatAboutPayload,
  formatInboxError,
  type ChatAboutStamp,
} from './inbox-browser'

function sendFailureMessage(status: ComposeStatus): string {
  if (status.kind === 'pty_died') {
    return status.hint?.trim() || 'Pinned chat PTY is not live (pty_died)'
  }
  if (status.kind === 'pty_stalled') {
    return status.hint?.trim() || 'Pinned chat PTY stalled'
  }
  if (status.kind === 'busy') {
    return status.hint?.trim() || status.reason?.trim() || 'Message was not delivered'
  }
  if (status.kind === 'error') return status.message
  return 'Message was not delivered'
}

export async function sendInboxChatAbout(args: {
  projectPath: string
  workspaceId: string | null | undefined
  stamp: ChatAboutStamp
  note: string
}): Promise<void> {
  const note = args.note.trim()
  if (!note) return
  const workspaceId = typeof args.workspaceId === 'string' ? args.workspaceId.trim() : ''
  if (workspaceId) activateProject(workspaceId)
  const ensured = await daemonCliPost<{ sessionId?: unknown }>('workspace/ensure-pinned-chat', {
    project: args.projectPath,
  })
  const sessionId = typeof ensured?.sessionId === 'string' ? ensured.sessionId.trim() : ''
  if (!sessionId) {
    throw new Error('Pinned chat did not return sessionId')
  }
  const text = formatChatAboutPayload(args.stamp, note)
  const resp = await daemonCliPost<MsgResponse>('terminal/send-message', {
    session_id: sessionId,
    text,
  })
  const status = mapMsgResponseToStatus(resp)
  if (status.kind !== 'delivered') {
    throw new Error(sendFailureMessage(status))
  }
}

export function InboxChatAboutDialog({
  stamp,
  projectPath,
  workspaceId,
  onClose,
}: {
  stamp: ChatAboutStamp
  projectPath: string
  workspaceId: string | null
  onClose: () => void
}): React.JSX.Element {
  const [frozen] = useState(stamp)
  const [note, setNote] = useState('')
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      if (!sending) onClose()
    }
    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [onClose, sending])

  const submit = async (): Promise<void> => {
    const trimmed = note.trim()
    if (!trimmed || sending) return
    setSending(true)
    setError(null)
    setNote('')
    try {
      await sendInboxChatAbout({
        projectPath,
        workspaceId,
        stamp: frozen,
        note: trimmed,
      })
      onClose()
    } catch (err) {
      setError(formatInboxError(err))
      setNote((cur) => (cur.length === 0 ? trimmed : cur))
    } finally {
      setSending(false)
    }
  }

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>): void => {
    if (!shouldSendOnKey({
      key: e.key,
      shiftKey: e.shiftKey,
      isComposing: e.nativeEvent.isComposing,
    })) return
    e.preventDefault()
    void submit()
  }

  const dismiss = (): void => {
    if (!sending) onClose()
  }

  return (
    <>
      <DialogScrim
        data-testid="inbox-chat-about-scrim"
        onMouseDown={(e) => {
          e.stopPropagation()
          dismiss()
        }}
      />
      <DialogFrame
        data-testid="inbox-chat-about-dialog"
        role="dialog"
        aria-label="Chat about this"
        onMouseDown={(e) => e.stopPropagation()}
        style={{
          width: 420,
          maxWidth: 'calc(100vw - 48px)',
          maxHeight: 'calc(100vh - 96px)',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div className="text-sm font-semibold text-[var(--color-text-primary)] truncate pr-2">
            Chat about this
          </div>
          <button
            type="button"
            onClick={dismiss}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors flex-shrink-0"
            title="Close (Esc)"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <line x1="2" y1="2" x2="10" y2="10" />
              <line x1="10" y1="2" x2="2" y2="10" />
            </svg>
          </button>
        </div>

        <div className="px-4 py-3 flex flex-col gap-2 min-h-0">
          <div className="text-[11px] text-[var(--color-text-muted)] font-mono break-all">
            {chatAboutStampLine(frozen)}
          </div>
          <textarea
            autoFocus
            data-testid="inbox-chat-about-note"
            value={note}
            disabled={sending}
            onChange={(e) => setNote(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Note for pinned Chat"
            className="w-full min-h-[88px] resize-y bg-[var(--color-bg)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 border border-[var(--color-border)] outline-none"
          />
          {error && (
            <div
              role="alert"
              data-testid="inbox-chat-about-error"
              className="text-[11px] text-[var(--color-status-error-soft)]"
            >
              {error}
            </div>
          )}
        </div>

        <div className="px-4 py-3 border-t border-[var(--color-border)] flex justify-end gap-2">
          <button
            type="button"
            data-testid="inbox-chat-about-cancel"
            onClick={dismiss}
            disabled={sending}
            className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer disabled:opacity-40"
          >
            Cancel
          </button>
          <button
            type="button"
            data-testid="inbox-chat-about-send"
            disabled={!note.trim() || sending}
            onClick={() => { void submit() }}
            className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 disabled:opacity-40 cursor-pointer"
          >
            Send
          </button>
        </div>
      </DialogFrame>
    </>
  )
}

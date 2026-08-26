import { useCallback, useState, type JSX, type KeyboardEvent } from 'react'
import { useOverlayThread } from './useOverlayThread'
import type { OverlayThreadItem } from './overlayThread'

interface ThreadOverlayPaneProps {
  addr: string
  conversationId: string | null
}

export function ThreadOverlayPane({
  addr,
  conversationId,
}: ThreadOverlayPaneProps): JSX.Element {
  const { items, error, posting, post } = useOverlayThread({
    addr,
    conversationId,
    enabled: true,
  })
  const [draft, setDraft] = useState('')

  const send = useCallback(async () => {
    const text = draft
    if (!text.trim() || posting) return
    setDraft('')
    try {
      await post(text)
    } catch {
      setDraft(text)
    }
  }, [draft, posting, post])

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void send()
    }
  }

  return (
    <div
      className="h-full flex flex-col min-h-0 bg-[var(--color-bg)]"
      data-testid="thread-overlay-pane"
    >
      <div className="flex-1 min-h-0 overflow-y-auto px-3 py-2 space-y-2">
        {error && (
          <div className="text-[11px] text-[var(--color-text-muted)]">{error}</div>
        )}
        {!error && items.length === 0 && (
          <div className="text-[11px] text-[var(--color-text-muted)]">
            No overlay posts yet. Compose below — this is not PTY inject.
          </div>
        )}
        {items.map((it) => (
          <ThreadItemRow key={it.id} item={it} />
        ))}
      </div>
      <div
        className="flex-shrink-0 border-t border-[var(--color-border)] px-3 py-2"
        data-compose-bar=""
      >
        <textarea
          data-testid="thread-compose"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
          disabled={posting || !addr.trim()}
          placeholder="Message the thread (not the terminal)"
          rows={1}
          className="w-full resize-none bg-transparent text-[12px] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none"
        />
      </div>
    </div>
  )
}

function ThreadItemRow({ item }: { item: OverlayThreadItem }): JSX.Element {
  return (
    <div data-testid="thread-item" data-kind={item.doc.kind} data-seq={item.seq}>
      <div className="text-[10px] text-[var(--color-text-muted)]">
        {item.doc.from || 'unknown'} · seq {item.seq}
      </div>
      <div className="text-[12px] text-[var(--color-text-primary)] whitespace-pre-wrap">
        {item.doc.body || ''}
      </div>
    </div>
  )
}

import { useEffect, useLayoutEffect, useRef, useState, type JSX } from 'react'
import { ChatMessage } from '@/components/common/ChatMessage'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { useSettingsStore } from '@/stores/settings'
import { useOverlayChatter } from './useOverlayChatter'
import type { OverlayDoc, OverlayThreadItem } from './overlayThread'

interface ChatterOverlayPaneProps {
  addr: string
  conversationId: string | null
  /** Pause the relative-time interval when the pane is display:none. */
  active?: boolean
}

/** A2A mailbox — no compose. Thread vs Terminal still own Message-the-agent. */
export function ChatterOverlayPane({
  addr,
  conversationId,
  active = true,
}: ChatterOverlayPaneProps): JSX.Element {
  const { items, error, hasMore, loadOlder, loadingOlder } = useOverlayChatter({
    addr,
    conversationId,
    enabled: true,
  })

  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 12
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    if (!active) return
    setNowSec(Math.floor(Date.now() / 1000))
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [active])

  const listRef = useRef<HTMLDivElement>(null)
  const didInitialScroll = useRef(false)
  const prevHeightRef = useRef<number | null>(null)

  useEffect(() => {
    didInitialScroll.current = false
  }, [addr])

  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    if (!didInitialScroll.current && items.length > 0) {
      el.scrollTop = el.scrollHeight
      didInitialScroll.current = true
      return
    }
    if (prevHeightRef.current != null) {
      el.scrollTop += el.scrollHeight - prevHeightRef.current
      prevHeightRef.current = null
    }
  }, [items])

  function requestOlder(): void {
    if (!hasMore || loadingOlder) return
    const el = listRef.current
    if (el) prevHeightRef.current = el.scrollHeight
    void loadOlder()
  }

  return (
    <div
      className="h-full flex flex-col min-h-0 bg-[var(--color-bg)]"
      data-testid="chatter-overlay-pane"
    >
      <div
        ref={listRef}
        className="flex-1 min-h-0 overflow-y-auto px-2 py-2"
        onScroll={(e) => {
          if (e.currentTarget.scrollTop <= 16) requestOlder()
        }}
      >
        {hasMore && (
          <button
            type="button"
            data-testid="overlay-load-older"
            disabled={loadingOlder}
            onClick={() => requestOlder()}
            className="w-full mb-2 px-2 py-1 text-[11px] text-[var(--color-text-muted)] border border-[var(--color-border)]"
          >
            {loadingOlder ? 'Loading…' : 'Load older'}
          </button>
        )}
        {error && (
          <div className="text-[11px] text-[var(--color-text-muted)] px-2.5">{error}</div>
        )}
        {!error && items.length === 0 && (
          <div className="text-[11px] text-[var(--color-text-muted)] px-2.5">
            No agent-to-agent messages yet.
          </div>
        )}
        <div className="flex flex-col gap-2.5">
          {items.map((it) => (
            <ChatterItemRow
              key={it.id}
              item={it}
              addr={addr}
              nowSec={nowSec}
              fontSize={editorFontSize}
            />
          ))}
        </div>
      </div>
    </div>
  )
}

export function chatterAuthor(doc: OverlayDoc): string {
  const from = doc.from.trim() || 'unknown'
  const to = (doc.to ?? '').trim()
  return to ? `${from} → ${to}` : from
}

export function ChatterItemRow({
  item,
  addr,
  nowSec,
  fontSize,
}: {
  item: OverlayThreadItem
  addr: string
  nowSec?: number
  fontSize?: number
}): JSX.Element {
  const owner = item.doc.from === addr
  const timeLabel = formatRelativeTime(
    item.doc.created_at,
    nowSec ?? Math.floor(Date.now() / 1000),
  )
  return (
    <div data-testid="chatter-item" data-kind={item.doc.kind} data-seq={item.seq}>
      <ChatMessage
        author={chatterAuthor(item.doc)}
        isOwner={owner}
        timeLabel={timeLabel}
        body={item.doc.body || ''}
        fontSize={fontSize}
      />
    </div>
  )
}

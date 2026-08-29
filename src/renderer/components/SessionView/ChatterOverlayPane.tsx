import { useEffect, useState, type JSX } from 'react'
import { ChatMessage } from '@/components/common/ChatMessage'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { useSettingsStore } from '@/stores/settings'
import { useOverlayChatter } from './useOverlayChatter'
import type { OverlayDoc, OverlayThreadItem } from './overlayThread'

interface ChatterOverlayPaneProps {
  addr: string
  conversationId: string | null
}

/** A2A mailbox — no compose. Thread vs Terminal still own Message-the-agent. */
export function ChatterOverlayPane({
  addr,
  conversationId,
}: ChatterOverlayPaneProps): JSX.Element {
  const { items, error } = useOverlayChatter({
    addr,
    conversationId,
    enabled: true,
  })

  const editorFontSize = useSettingsStore((s) => s.editor.fontSize) || 12
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [])

  return (
    <div
      className="h-full flex flex-col min-h-0 bg-[var(--color-bg)]"
      data-testid="chatter-overlay-pane"
    >
      <div className="flex-1 min-h-0 overflow-y-auto px-2 py-2">
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

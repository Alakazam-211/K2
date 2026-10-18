// 0.46.1 Zen defaults (thread-read-v1): what a built-in Conversation widget
// shows of the person's read state on the agent's Thread.
//
//   - It marks the Thread read while it is looked at (the widget shows a
//     ready conversation, the window is visible and focused), the same
//     rule as the desktop Thread pane (`useAutoMarkRead`), so reading in
//     a Garden clears the unread dot on every device.
//   - "N waiting" in the header: choice and secret cards waiting on you.
//     A click scrolls to the first one. Reading never clears it; answering
//     does (the daemon's `waiting`).
//   - With `mute = true` (Garden 1 turns it on): a bell to mute or unmute
//     this Thread. A muted Thread keeps its count as a grey dot and leaves
//     the badge totals; pushes stop.
//
// Only the window's server keeps an unread store; on another server, or
// one without `thread-read-v1`, nothing is posted and the bell is hidden
// (the waiting chip still counts the cards in view).

import { useCallback } from 'react'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { unreadCountText, unreadSupported, useAutoMarkRead, useUnreadEntry, useUnreadStore } from '@/stores/unread'
import type { ZenThreadView } from '@/lib/zen/zen-data'

/** A choice or secret card still waiting on an answer. */
export function zenCardWaiting(item: OverlayThreadItem): boolean {
  const d = item.doc
  if (d.kind === 'choice') return d.choice?.status === 'pending'
  if (d.kind === 'secret') return d.secret?.status === 'pending'
  return false
}

/** The newest seq a view holds (null when it holds none). */
export function zenNewestSeq(items: readonly OverlayThreadItem[]): number | null {
  let max: number | null = null
  for (const it of items) if (typeof it.seq === 'number' && (max === null || it.seq > max)) max = it.seq
  return max
}

/** Mark the shown Thread read while it is looked at (window server only). */
export function useZenThreadAutoRead(view: ZenThreadView | null, shown: boolean): void {
  const items = view?.items ?? []
  useAutoMarkRead({
    kind: 'thread',
    id: view?.windowServer ? (view.conversationId ?? null) : null,
    newestSeq: zenNewestSeq(items),
    visible: shown && view?.phase === 'ready' && view.windowServer === true,
  })
}

/** The person's read state on the shown Thread (window server only). */
export function useZenThreadEntry(view: ZenThreadView | null): ReturnType<typeof useUnreadEntry> {
  return useUnreadEntry('thread', view?.windowServer ? (view.conversationId ?? null) : null)
}

/** "2 waiting": cards waiting on you; the daemon's count, or the cards in
 *  view when it can't say. Null at none. A click scrolls to the first. */
export function ZenWaitingChip({
  waiting,
  onShow,
}: {
  waiting: number
  onShow(): void
}): React.JSX.Element | null {
  const text = unreadCountText(waiting)
  if (text === null) return null
  return (
    <button
      type="button"
      data-zen-waiting={waiting}
      onClick={onShow}
      title={`${waiting} card${waiting === 1 ? '' : 's'} waiting on you`}
      className="no-drag flex flex-shrink-0 cursor-pointer items-center"
      style={{
        gap: 5,
        padding: '2px 8px',
        borderRadius: 999,
        border: 0,
        font: 'inherit',
        fontSize: '0.75em',
        fontWeight: 600,
        background: 'var(--zen-needs-you)',
        color: 'var(--zen-accent-text)',
      }}
    >
      {text} waiting
    </button>
  )
}

function BellIcon({ off }: { off: boolean }): React.JSX.Element {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="square" aria-hidden>
      <path d="M6 9a6 6 0 0 1 12 0c0 7 3 8 3 8H3s3-1 3-8" />
      <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
      {off && <path d="M3 3l18 18" />}
    </svg>
  )
}

/** Mute / unmute the shown Thread (`mute` prop). Hidden where the window's
 *  server can't keep read state. */
export function ZenMuteButton({ view }: { view: ZenThreadView | null }): React.JSX.Element | null {
  const entry = useZenThreadEntry(view)
  const id = view?.windowServer ? (view.conversationId ?? null) : null
  const muted = entry?.muted === true
  const toggle = useCallback(() => {
    if (!id) return
    void useUnreadStore
      .getState()
      .setMuted('thread', id, !muted)
      .catch((err: unknown) => console.warn('[zen] mute failed:', err))
  }, [id, muted])
  if (!id || !unreadSupported()) return null
  return (
    <button
      type="button"
      data-zen-mute={muted ? 'on' : 'off'}
      aria-pressed={muted}
      aria-label={muted ? 'Unmute this Thread' : 'Mute this Thread'}
      title={muted ? 'Muted: no pushes, left out of your badge. Click to unmute.' : 'Mute this Thread'}
      onClick={toggle}
      className="no-drag flex flex-shrink-0 cursor-pointer items-center justify-center"
      style={{
        width: 28,
        height: 28,
        borderRadius: 999,
        border: 0,
        background: muted ? 'var(--zen-surface-raised)' : 'transparent',
        color: muted ? 'var(--zen-text)' : 'var(--zen-text-muted)',
      }}
    >
      <BellIcon off={muted} />
    </button>
  )
}

import { lazy, Suspense, useLayoutEffect, useRef, useState, type JSX } from 'react'
import { useRoom } from '@/components/Room/RoomContext'

// As ThreadOverlayPane: the avatar pulls in the projects and Home avatar stores.
const AgentAvatar = lazy(() =>
  import('@/components/Thread/ThreadSenderAvatars').then((m) => ({ default: m.AgentAvatar })),
)

// Thread view's header centre (prd-thread-view-focus-v1 §4): the agent's
// picture above its name, lifted from the phone's chat header. Thread view
// only; other views keep the plain title. No status here, as on the phone:
// the working strip carries it. A room on another server adds that server's
// name under the agent's.

export const IDENTITY_PICTURE_PX = 32
export const IDENTITY_PICTURE_COMPACT_PX = 20
/** Below either, the header drops to one line. */
export const IDENTITY_COMPACT_BELOW_H = 420
export const IDENTITY_COMPACT_BELOW_W = 360

/** One line in a short or narrow pane. A 0 size (not laid out) isn't small. */
export function identityCompact(width: number, height: number): boolean {
  return (height > 0 && height < IDENTITY_COMPACT_BELOW_H) || (width > 0 && width < IDENTITY_COMPACT_BELOW_W)
}

export function ThreadViewIdentity({ name, addr }: { name: string; addr: string }): JSX.Element {
  const room = useRoom()
  const rootRef = useRef<HTMLDivElement>(null)
  const [compact, setCompact] = useState(false)

  // The pane, not the window: the header's parent (a column or a split pane
  // counts). The header's own height doesn't change the pane's size, so
  // switching layouts can't feed back into this measure.
  useLayoutEffect(() => {
    const pane = rootRef.current?.closest('[data-session-header]')?.parentElement
    if (!pane) return
    const measure = (): void => {
      const rect = pane.getBoundingClientRect()
      setCompact(identityCompact(rect.width, rect.height))
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    ro.observe(pane)
    return () => ro.disconnect()
  }, [])

  const size = compact ? IDENTITY_PICTURE_COMPACT_PX : IDENTITY_PICTURE_PX
  const server = room.scope.isPrimary ? null : room.scope.label

  return (
    <div
      ref={rootRef}
      data-testid="thread-view-identity"
      data-compact={compact ? '' : undefined}
      title={compact && server ? server : undefined}
      className={`min-w-0 flex items-center justify-center text-center ${
        compact ? 'flex-row gap-1.5 py-1' : 'flex-col gap-0.5 py-1.5'
      }`}
    >
      <span
        data-testid="thread-view-picture"
        className="flex-shrink-0 block"
        style={{ width: size, height: size, overflow: 'hidden', borderRadius: 'var(--radius-pill)' }}
      >
        <Suspense fallback={null}>
          <AgentAvatar addr={addr} label={name} host={room.scope.isPrimary ? null : room.scope.hostKey} size={size} />
        </Suspense>
      </span>
      <span
        data-testid="thread-view-name"
        className="min-w-0 max-w-full truncate font-semibold text-[var(--color-text-primary)]"
        style={{ fontSize: 'var(--type-lg)', lineHeight: 1.2 }}
      >
        {name}
      </span>
      {server && !compact ? (
        <span
          data-testid="thread-view-server"
          className="min-w-0 max-w-full truncate text-[var(--color-text-muted)]"
          style={{ fontSize: 'var(--type-xs)', lineHeight: 1.2 }}
        >
          {server}
        </span>
      ) : null}
    </div>
  )
}

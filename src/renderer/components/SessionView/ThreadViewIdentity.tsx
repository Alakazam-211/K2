import { lazy, Suspense, useLayoutEffect, useRef, useState, type JSX } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { floatProps, threadFloatsHostOf } from './threadFloats'

// As ThreadOverlayPane: the avatar pulls in the projects and Home avatar stores.
const AgentAvatar = lazy(() =>
  import('@/components/Thread/ThreadSenderAvatars').then((m) => ({ default: m.AgentAvatar })),
)

// Thread view's header centre (prd-thread-view-focus-v1 §4): the agent's
// picture above its name, lifted from the phone's chat header. Thread view
// only; other views keep the plain title. No status here, as on the phone:
// the working strip carries it. A room on another server adds that server's
// name under the agent's.

export const IDENTITY_PICTURE_PX = 36
export const IDENTITY_PICTURE_COMPACT_PX = 20
/** Below either, the header drops to one line. */
export const IDENTITY_COMPACT_BELOW_H = 420
export const IDENTITY_COMPACT_BELOW_W = 360

/** One line in a short or narrow pane. A 0 size (not laid out) isn't small. */
export function identityCompact(width: number, height: number): boolean {
  return (height > 0 && height < IDENTITY_COMPACT_BELOW_H) || (width > 0 && width < IDENTITY_COMPACT_BELOW_W)
}

/** The agent's picture, round, lifted over what is under it (and edged in
 *  Terminal), from the room's server. The header draws it at 36px; an empty
 *  Thread draws it larger over its message box (ThreadEmptyStart). */
export function ThreadViewPicture({
  name,
  addr,
  size,
  testId = 'thread-view-picture',
}: {
  name: string
  addr: string
  size: number
  testId?: string
}): JSX.Element {
  const room = useRoom()
  return (
    <span
      data-testid={testId}
      data-k2-float-picture=""
      data-k2-chamfer="control"
      className="flex-shrink-0 block"
      style={{ width: size, height: size, overflow: 'hidden', borderRadius: 'var(--radius-pill)' }}
    >
      <Suspense fallback={null}>
        <AgentAvatar addr={addr} label={name} host={room.scope.isPrimary ? null : room.scope.hostKey} size={size} />
      </Suspense>
    </span>
  )
}

/** `bar`: split view's bar, always the one-line pill. */
export function ThreadViewIdentity({ name, addr, bar = false }: { name: string; addr: string; bar?: boolean }): JSX.Element {
  const room = useRoom()
  const rootRef = useRef<HTMLDivElement>(null)
  const [measuredCompact, setCompact] = useState(false)
  const compact = bar || measuredCompact

  // The pane, not the window: the header's host (a column or a split pane
  // counts). The header's own height doesn't change the pane's size, so
  // switching layouts can't feed back into this measure.
  useLayoutEffect(() => {
    if (bar) return
    // The floats' header: the session's (Thread and Chatter views) or split
    // view's Thread column's own (SplitColumnFloats).
    const header = rootRef.current?.closest('[data-thread-floats], [data-session-header]')
    const pane = header ? threadFloatsHostOf(header) : null
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
  }, [bar])

  const size = compact ? IDENTITY_PICTURE_COMPACT_PX : IDENTITY_PICTURE_PX
  const server = room.scope.isPrimary ? null : room.scope.label

  const picture = <ThreadViewPicture name={name} addr={addr} size={size} />
  const nameLine = (
    <span
      data-testid="thread-view-name"
      className="min-w-0 max-w-full truncate font-semibold text-[var(--color-text-primary)]"
      style={{ fontSize: 'var(--type-lg)', lineHeight: 1.2 }}
      title={name}
    >
      {name}
    </span>
  )

  // The picture sits on the name's object and overlaps its top edge, as on
  // the phone. In one line it goes inside.
  return (
    <div
      ref={rootRef}
      data-testid="thread-view-identity"
      data-thread-view-identity=""
      data-compact={compact ? '' : undefined}
      title={compact && server ? server : undefined}
      className={`min-w-0 max-w-full flex items-center justify-center text-center ${compact ? 'flex-row' : 'flex-col'}`}
    >
      {compact ? null : picture}
      <div
        {...floatProps(true, 'name')}
        className={`min-w-0 max-w-full flex items-center ${compact ? 'flex-row gap-1.5' : 'flex-col'}`}
      >
        {compact ? picture : null}
        {nameLine}
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
    </div>
  )
}

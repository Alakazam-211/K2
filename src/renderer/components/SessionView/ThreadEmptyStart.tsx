import { forwardRef, type JSX } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { ThreadViewPicture } from './ThreadViewIdentity'

// An empty Thread starts centred (Rosson, 2026-11-10: "place the message the
// agent tile in the center along with the agents name and image"): the
// agent's picture and name, and the message box right under them, as one
// group in the middle of the Thread's column. On the first message (yours or
// the agent's) the box drifts down to its place and the group fades
// (ThreadOverlayColumn). The header's own picture and name step aside while
// this shows (globals.css, "An empty Thread's start").

/** Between the name and the message box. */
export const EMPTY_START_GAP_PX = 20
/** The least room kept under the floating header. */
export const EMPTY_START_PAD_PX = 12
/** The picture: as on the phone's empty chat, smaller in a small column. */
export const EMPTY_START_PICTURE_PX = 72
export const EMPTY_START_PICTURE_SMALL_PX = 56
export const EMPTY_START_SMALL_BELOW_W = 420
export const EMPTY_START_SMALL_BELOW_H = 560
/** Where the spare height goes: a little more under the group than over
 *  it, so it sits at the eye's centre rather than the box's. */
const EMPTY_START_BIAS = 0.45

/** Where the group goes. `top`: the picture's top in the column; `lift`:
 *  how far the message box rises from its bottom place. Not `fits`: the
 *  column is too short for it, so the box stays at the bottom and the
 *  Thread draws its plain empty line. */
export interface EmptyStartPlace {
  fits: boolean
  top: number
  lift: number
  small: boolean
}

/** The group's place in a column `height` tall whose header floats take
 *  `inset`, for a message box `box` tall and a picture-and-name `hero` tall.
 *  Null until the column is laid out. */
export function emptyStartPlace(o: {
  width: number
  height: number
  inset: number
  box: number
  hero: number
}): EmptyStartPlace | null {
  if (o.height <= 0 || o.box <= 0) return null
  const small = o.width < EMPTY_START_SMALL_BELOW_W || o.height < EMPTY_START_SMALL_BELOW_H
  const total = o.hero + EMPTY_START_GAP_PX + o.box
  const minTop = Math.ceil(o.inset) + EMPTY_START_PAD_PX
  // Centred; short of room, top-aligned under the floats.
  const top = Math.max(minTop, Math.round((o.height - total) * EMPTY_START_BIAS))
  if (top + total > o.height) return { fits: false, top: 0, lift: 0, small }
  const lift = Math.max(0, Math.round(o.height - o.box - (top + o.hero + EMPTY_START_GAP_PX)))
  return { fits: true, top, lift, small }
}

export function sameEmptyStartPlace(a: EmptyStartPlace | null, b: EmptyStartPlace | null): boolean {
  if (a === b) return true
  if (!a || !b) return false
  return a.fits === b.fits && a.top === b.top && a.lift === b.lift && a.small === b.small
}

/** The picture and name over the message box. Drawn while the Thread is
 *  empty and while the box drifts down; `shown` only while centred. */
export const ThreadEmptyStart = forwardRef<
  HTMLDivElement,
  { name: string; addr: string; top: number; small: boolean; shown: boolean; maxWidth?: string }
>(function ThreadEmptyStart({ name, addr, top, small, shown, maxWidth }, ref): JSX.Element {
  const room = useRoom()
  const server = room.scope.isPrimary ? null : room.scope.label
  const size = small ? EMPTY_START_PICTURE_SMALL_PX : EMPTY_START_PICTURE_PX
  return (
    <div
      ref={ref}
      data-testid="thread-empty-start"
      data-thread-empty-start=""
      data-shown={shown ? '' : undefined}
      aria-hidden={shown ? undefined : true}
      className="absolute left-0 right-0 flex flex-col items-center text-center pointer-events-none"
      style={{ top, zIndex: 1, marginInline: 'auto', maxWidth, paddingInline: 'var(--zen-gap, 12px)' }}
    >
      <ThreadViewPicture name={name} addr={addr} size={size} testId="thread-empty-start-picture" />
      <span
        data-testid="thread-empty-start-name"
        className="mt-2.5 min-w-0 max-w-full truncate font-semibold text-[var(--color-text-primary)]"
        style={{ fontSize: 'var(--type-xl)', lineHeight: 1.25 }}
        title={name}
      >
        {name}
      </span>
      {server ? (
        <span
          data-testid="thread-empty-start-server"
          className="min-w-0 max-w-full truncate text-[var(--color-text-muted)]"
          style={{ fontSize: 'var(--type-xs)', lineHeight: 1.3 }}
        >
          {server}
        </span>
      ) : null}
    </div>
  )
})

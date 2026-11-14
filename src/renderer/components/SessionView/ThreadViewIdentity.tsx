import { lazy, Suspense, useLayoutEffect, useRef, useState, type JSX } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { THREAD_FLOATS_GRID_ATTR, floatProps, threadFloatsHostOf } from './threadFloats'

// As ThreadOverlayPane: the avatar pulls in the projects and Home avatar stores.
const AgentAvatar = lazy(() =>
  import('@/components/Thread/ThreadSenderAvatars').then((m) => ({ default: m.AgentAvatar })),
)

// Thread view's header centre (prd-thread-view-focus-v1 §4): the agent's
// picture above its name, lifted from the phone's chat header. Thread view
// only; other views keep the plain title. No status here, as on the phone:
// the working strip carries it. A room on another server adds that server's
// name under the agent's.
//
// The name keeps its size (Rosson 2026-11-14, as on the phone): as the pane
// narrows, the name's object grows past the picture's column, under the
// objects beside it (where it melts into them in Glass, or joins them in
// Terminal: ThreadFloatMerge), out to the row's edges. Only then does it give
// way, in this order (`identityFit`): its padding tightens, the server line
// shortens (when it is the longer line), and last of all the name, never
// below IDENTITY_NAME_MIN_PX, with the whole name on hover.

export const IDENTITY_PICTURE_PX = 36
export const IDENTITY_PICTURE_COMPACT_PX = 20
/** Below this, the header drops to one line. A narrow pane keeps the
 *  picture above the name: the name has the whole row's width under it. */
export const IDENTITY_COMPACT_BELOW_H = 420

/** One line in a short pane. A 0 size (not laid out) isn't small. */
export function identityCompact(height: number): boolean {
  return height > 0 && height < IDENTITY_COMPACT_BELOW_H
}

/** The name's object's padding at each side, roomy and once out of room. */
export const IDENTITY_PAD_PX = { roomy: 14, snug: 8 } as const
/** In one line the picture sits inside, at the left: its padding, the
 *  picture and the gap after it, then the right padding. */
const COMPACT_LEAD_PX = 4 + IDENTITY_PICTURE_COMPACT_PX + 6
export const IDENTITY_COMPACT_PAD_PX = { roomy: 12, snug: 8 } as const
/** The least of the name that shows before anything else is out of room
 *  (about six characters). */
export const IDENTITY_NAME_MIN_PX = 72

export type IdentityFit = {
  pad: 'roomy' | 'snug'
  /** The object's width, px. */
  width: number
  serverClips: boolean
  nameClips: boolean
}

/**
 * How the name's object fits `room` px (the row's width; in one line, the
 * room between the objects beside it). The text never shrinks; the object
 * gives way around it: first its padding, then the server line, then the
 * name, down to IDENTITY_NAME_MIN_PX.
 */
export function identityFit({
  name,
  server = 0,
  room,
  compact = false,
}: {
  name: number
  server?: number
  room: number
  compact?: boolean
}): IdentityFit {
  const chrome = (pad: 'roomy' | 'snug'): number =>
    compact ? COMPACT_LEAD_PX + IDENTITY_COMPACT_PAD_PX[pad] : 2 * IDENTITY_PAD_PX[pad]
  const text = Math.max(name, compact ? 0 : server)
  for (const pad of ['roomy', 'snug'] as const) {
    if (text + chrome(pad) <= room) return { pad, width: text + chrome(pad), serverClips: false, nameClips: false }
  }
  const inner = Math.max(room - chrome('snug'), Math.min(name, IDENTITY_NAME_MIN_PX))
  return {
    pad: 'snug',
    width: inner + chrome('snug'),
    serverClips: !compact && server > inner,
    nameClips: name > inner,
  }
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

const cssPx = (v: string): number => {
  const n = parseFloat(v)
  return Number.isFinite(n) ? n : 0
}

/** The content width of an element, CSS px (client sizes are CSS px at any
 *  app zoom). */
function contentWidth(el: HTMLElement): number {
  const cs = getComputedStyle(el)
  return el.clientWidth - cssPx(cs.paddingLeft) - cssPx(cs.paddingRight)
}

/** A row of objects' own width (a group is stretched wider than them). */
function objectsWidth(group: Element | null): number {
  if (!(group instanceof HTMLElement) || group.childElementCount === 0) return 0
  const kids = [...group.children].filter((k) => k instanceof HTMLElement && k.offsetWidth > 0) as HTMLElement[]
  if (kids.length === 0) return 0
  const gap = cssPx(getComputedStyle(group).columnGap)
  return kids.reduce((n, k) => n + k.offsetWidth, 0) + gap * (kids.length - 1)
}

/** `bar`: split view's bar, always the one-line pill. */
export function ThreadViewIdentity({ name, addr, bar = false }: { name: string; addr: string; bar?: boolean }): JSX.Element {
  const room = useRoom()
  const rootRef = useRef<HTMLDivElement>(null)
  const plateRef = useRef<HTMLDivElement>(null)
  const nameRef = useRef<HTMLSpanElement>(null)
  const serverRef = useRef<HTMLSpanElement>(null)
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
      setCompact(identityCompact(rect.height))
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    ro.observe(pane)
    return () => ro.disconnect()
  }, [bar])

  const server = room.scope.isPrimary ? null : room.scope.label

  // The fit (identityFit), in the floats' row only. Written to the DOM here,
  // not React state: it follows a window resize every frame, and nothing it
  // writes changes what it measures (the row, the objects beside it, the
  // text's own width), so it can't feed back.
  useLayoutEffect(() => {
    const root = rootRef.current
    const plate = plateRef.current
    const grid = root?.closest<HTMLElement>(`[${THREAD_FLOATS_GRID_ATTR}]`)
    if (bar || !root || !plate || !grid) return
    const unplate = (): void => {
      root.removeAttribute('data-plate')
      for (const v of ['--k2-plate-w', '--k2-plate-ml', '--k2-plate-mr']) root.style.removeProperty(v)
    }
    const unfit = (): void => {
      root.removeAttribute('data-fit')
      unplate()
      nameRef.current?.style.removeProperty('min-width')
    }
    const fit = (): void => {
      const nameEl = nameRef.current
      const rowWidth = contentWidth(grid)
      if (!nameEl || rowWidth <= 0) {
        unfit()
        return
      }
      // The text's own widths (a shortened line's scrollWidth is its whole
      // text), plus a px so a fraction never shortens it.
      const nameW = nameEl.scrollWidth + 1
      const serverW = serverRef.current ? serverRef.current.scrollWidth + 1 : 0
      // The object's own edge (a hairline, or Glass's half-pixel rim) sits
      // inside its width too.
      const ps = getComputedStyle(plate)
      const edges = cssPx(ps.borderLeftWidth) + cssPx(ps.borderRightWidth)
      const oneLine = root.hasAttribute('data-compact')
      if (oneLine) {
        // In one line the name sits between the objects beside it.
        const column = root.parentElement
        const gap = cssPx(getComputedStyle(grid).columnGap)
        const room =
          rowWidth - objectsWidth(column?.previousElementSibling ?? null) - objectsWidth(column?.nextElementSibling ?? null) - 2 * gap
        const f = identityFit({ name: nameW, room: room - edges, compact: true })
        root.setAttribute('data-fit', f.pad)
        unplate()
        nameEl.style.minWidth = `${Math.min(nameW, IDENTITY_NAME_MIN_PX)}px`
        return
      }
      nameEl.style.removeProperty('min-width')
      const fitted = identityFit({ name: nameW, server: serverW, room: rowWidth - edges })
      const f = { ...fitted, width: fitted.width + edges }
      // Centred under the picture, kept inside the row's edges (the picture
      // sits off-centre only when the row is out of room). Rects are window
      // px; offsetWidth over the rect's width turns them into CSS px at any
      // app zoom.
      const g = grid.getBoundingClientRect()
      const r = root.getBoundingClientRect()
      const scale = g.width > 0 ? grid.offsetWidth / g.width : 1
      const rowLeft = cssPx(getComputedStyle(grid).paddingLeft)
      const centre = (r.left + r.width / 2 - g.left) * scale
      const left = Math.max(rowLeft, Math.min(centre - f.width / 2, rowLeft + rowWidth - f.width))
      const shift = left - (centre - f.width / 2)
      // Its margins take back all but the picture's width, so the column
      // stays the picture's: the picture stays centred, and the object
      // reaches out either side of it, over the columns beside it.
      const back = (IDENTITY_PICTURE_PX - f.width) / 2
      const round = (n: number): string => `${Math.round(n * 100) / 100}px`
      root.setAttribute('data-fit', f.pad)
      root.setAttribute('data-plate', '')
      root.style.setProperty('--k2-plate-w', round(f.width))
      root.style.setProperty('--k2-plate-ml', round(back + shift))
      root.style.setProperty('--k2-plate-mr', round(back - shift))
    }
    fit()
    // A font that loads late changes the text's width but no box's size (a
    // shortened name's box is its object's): fit again when it lands.
    const fonts = typeof document !== 'undefined' ? document.fonts : undefined
    let live = true
    void fonts?.ready.then(() => {
      if (live) fit()
    })
    fonts?.addEventListener?.('loadingdone', fit)
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(fit)
    ro?.observe(grid)
    ro?.observe(root)
    return () => {
      live = false
      fonts?.removeEventListener?.('loadingdone', fit)
      ro?.disconnect()
      unfit()
    }
  }, [bar, compact, name, server])

  const size = compact ? IDENTITY_PICTURE_COMPACT_PX : IDENTITY_PICTURE_PX

  const picture = <ThreadViewPicture name={name} addr={addr} size={size} />
  const nameLine = (
    <span
      ref={nameRef}
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
        ref={plateRef}
        {...floatProps(true, 'name')}
        className={`min-w-0 max-w-full flex items-center ${compact ? 'flex-row gap-1.5' : 'flex-col'}`}
      >
        {compact ? picture : null}
        {nameLine}
        {server && !compact ? (
          <span
            ref={serverRef}
            data-testid="thread-view-server"
            className="min-w-0 max-w-full truncate text-[var(--color-text-muted)]"
            style={{ fontSize: 'var(--type-xs)', lineHeight: 1.2 }}
            title={server}
          >
            {server}
          </span>
        ) : null}
      </div>
    </div>
  )
}

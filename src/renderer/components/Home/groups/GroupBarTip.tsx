// Home groups: the name tip over a group's icon in the bar (beside it in the
// rail): the group's name and its ⌥⌘N, and on keyboard focus how to edit
// groups. The icon's own label already says all of it, so the tip is only
// for the eye (aria-hidden). Portalled, kept 8 px inside the window, and
// it claims its box over a live Browser page (lib/browser-overlay). In
// Terminal it is cut like the top bar's floats (the control chamfer; its
// shadow on a wrapper, since a box-shadow can't follow a cut).

import { useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { clientRectToCss, cssViewport } from '@/lib/app-zoom'
import { useBrowserOverlay } from '@/lib/browser-overlay'

export interface GroupBarTipProps {
  anchor: HTMLElement
  name: string
  chord: string | null
  /** Keyboard focus: also says Shift F10 edits groups. */
  keyboard: boolean
  vertical: boolean
}

const GAP_PX = 8
const RAIL_GAP_PX = 10
const EDGE_PX = 8

export default function GroupBarTip({ anchor, name, chord, keyboard, vertical }: GroupBarTipProps): React.ReactPortal {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  // Beside the rail it can reach over the room: claim its box over a live
  // Browser page.
  useBrowserOverlay({ active: true, ref, kind: 'popover', watch: pos })

  useLayoutEffect(() => {
    const tip = ref.current
    if (!tip) return
    // CSS px throughout, so app zoom places it right (lib/app-zoom).
    const a = clientRectToCss(anchor.getBoundingClientRect())
    const w = tip.offsetWidth
    const h = tip.offsetHeight
    const { width: vw, height: vh } = cssViewport()
    let left = vertical ? a.right + RAIL_GAP_PX : a.left + a.width / 2 - w / 2
    let top = vertical ? a.top + (a.height - h) / 2 : a.top - h - GAP_PX
    left = Math.max(EDGE_PX, Math.min(left, vw - w - EDGE_PX))
    top = Math.max(EDGE_PX, Math.min(top, vh - h - EDGE_PX))
    setPos({ left: Math.round(left), top: Math.round(top) })
  }, [anchor, name, chord, keyboard, vertical])

  return createPortal(
    <div
      ref={ref}
      aria-hidden="true"
      data-group-bar-tip=""
      data-k2-chamfer-float="menu"
      className="pointer-events-none fixed z-[1000]"
      style={{ left: pos?.left ?? -9999, top: pos?.top ?? -9999 }}
    >
      <div data-group-float="tip" data-k2-chamfer="control" className="flex items-center gap-2 whitespace-nowrap px-2 py-1 text-[11px] text-[var(--color-text-primary)]">
        <span>{name}</span>
        {chord && <span className="text-[var(--color-text-secondary)]">{chord}</span>}
        {keyboard && <span className="text-[var(--color-text-secondary)]">· Shift F10 edits groups</span>}
      </div>
    </div>,
    document.body,
  )
}

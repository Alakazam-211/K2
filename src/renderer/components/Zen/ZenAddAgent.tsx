// Zen "Add agent" picker (Rosson 2026-10-04), drawn by K2 over the Zen page
// when a widget calls the bridge verb `agents.add` (`lib/zen/zen-add-agent`).
//
// It is the regular Home's picker, not a copy: `AddAgentPicker` (This server
// / From a server, the shared `SearchableAgentList`, the Home add path that
// also caches a remote row's picture), for the CURRENT Home. Adding writes
// the row into that Home, and the Agents widget, which reads the selected
// Home's rows, shows it at once. Adding keeps the picker open, like Home.
//
// Opens above the widget's anchor (left-aligned), else in the bottom-left
// corner. Registered as a K2 overlay, so the required-controls check never
// counts it as covering the page's controls. Closes on Esc, a click outside
// it (the anchor itself toggles), a Home switch, and when Zen leaves the
// window (this unmounts).

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { registerZenK2Overlay } from '@/lib/zen/zen-controls'
import { closeZenAddAgent, useZenAddAgentStore } from '@/lib/zen/zen-add-agent'
import { AddAgentPicker } from '@/components/Home/HomeAddPanels'

/** Picker width (CSS px). The Home panel insets 12px each side inside it. */
export const ZEN_ADD_AGENT_WIDTH_PX = 340
/** `AddAgentPicker`'s own `left-3` inset, so its left edge meets the anchor. */
const PANEL_INSET_PX = 12
const GAP_PX = 6
const CORNER_PX = 14

interface Spot {
  left: number
  top: number
}

function spotFor(anchor: HTMLElement | null): Spot {
  const vw = typeof window === 'undefined' ? 0 : window.innerWidth
  const vh = typeof window === 'undefined' ? 0 : window.innerHeight
  const maxLeft = Math.max(0, vw - ZEN_ADD_AGENT_WIDTH_PX)
  if (!anchor || !anchor.isConnected) {
    return { left: Math.min(maxLeft, Math.max(0, CORNER_PX - PANEL_INSET_PX)), top: vh - CORNER_PX }
  }
  const r = anchor.getBoundingClientRect()
  return { left: Math.min(maxLeft, Math.max(0, r.left - PANEL_INSET_PX)), top: r.top - GAP_PX }
}

export function ZenAddAgentPicker(): React.JSX.Element | null {
  const open = useZenAddAgentStore((s) => s.open)
  const anchor = useZenAddAgentStore((s) => s.anchor)
  const home = useHomesStore(selectedHome)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const [spot, setSpot] = useState<Spot>(() => spotFor(anchor))

  // Leaving Zen (this unmounts) closes it, so it never comes back on the
  // next Zen-on or lingers over the regular Home.
  useEffect(() => () => closeZenAddAgent(), [])

  // The picker is for the Home on screen: a Home switch closes it.
  const homeId = home.id
  const firstHome = useRef(homeId)
  useEffect(() => {
    if (firstHome.current === homeId) return
    firstHome.current = homeId
    closeZenAddAgent()
  }, [homeId])

  // Place it above the anchor now and on every resize.
  useLayoutEffect(() => {
    if (!open) return
    const place = (): void => setSpot(spotFor(anchor))
    place()
    window.addEventListener('resize', place)
    return () => window.removeEventListener('resize', place)
  }, [open, anchor])

  // K2 overlay for the control check.
  useEffect(() => {
    const el = boxRef.current
    if (!open || !el) return
    return registerZenK2Overlay(el)
  }, [open])

  // Esc and a click outside close it. The anchor is left to its own click
  // (it toggles), so a press on it doesn't close-then-reopen.
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      closeZenAddAgent()
    }
    const onDown = (e: MouseEvent): void => {
      const t = e.target
      if (!(t instanceof Node)) return
      if (boxRef.current?.contains(t)) return
      if (anchor?.contains(t)) return
      closeZenAddAgent()
    }
    window.addEventListener('keydown', onKey, true)
    document.addEventListener('mousedown', onDown, true)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      document.removeEventListener('mousedown', onDown, true)
    }
  }, [open, anchor])

  if (!open) return null
  return (
    <div
      ref={boxRef}
      data-zen-add-agent-picker={home.id}
      className="no-drag"
      style={{
        position: 'fixed',
        left: spot.left,
        top: spot.top,
        width: ZEN_ADD_AGENT_WIDTH_PX,
        height: 0,
        zIndex: 30,
      }}
    >
      <AddAgentPicker home={home} />
    </div>
  )
}

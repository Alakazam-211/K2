// Zen "Add agent" picker (Rosson 2026-10-04), drawn by K2 over the Zen page
// when a widget calls the bridge verb `agents.add` (`lib/zen/zen-add-agent`).
//
// It is the regular Home's picker, not a copy: `AddAgentPicker` (This server
// / From a server, the shared `SearchableAgentList`, the Home add path that
// also caches a remote row's picture), for the Home the calling Agents
// widget shows (prd-zen-gardens-v1 G33, G55) — never the window's selected
// Home, which never moves. Adding writes the row into that Home, and the
// Agents widget shows it at once. Adding closes the picker and opens that
// agent's conversation with the caret in its box (Rosson 2026-10-04).
//
// Opens above the widget's anchor (left-aligned), else in the bottom-left
// corner. Registered as a K2 overlay, so the required-controls check never
// counts it as covering the page's controls. Closes on Esc, a click outside
// it (the anchor itself toggles), that widget's Home changing, a Garden
// switch, and when Zen leaves the window (this unmounts).

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useHomesStore } from '@/stores/homes'
import { registerZenK2Overlay } from '@/lib/zen/zen-controls'
import { closeZenAddAgent, noteZenAgentAdded, useZenAddAgentStore } from '@/lib/zen/zen-add-agent'
import { useCurrentZenGarden } from '@/lib/zen/zen-gardens'
import { useZenGardenHomesStore } from '@/lib/zen/zen-garden-homes'
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
  const homeId = useZenAddAgentStore((s) => s.homeId)
  const openedIn = useZenAddAgentStore((s) => s.gardenId)
  const viewKey = useZenAddAgentStore((s) => s.viewKey)
  const pickedNow = useZenGardenHomesStore((s) => (viewKey ? (s.picks[viewKey] ?? null) : null))
  const home = useHomesStore((s) => s.homes.find((h) => h.id === homeId) ?? null)
  const gardenId = useCurrentZenGarden()?.id ?? null
  const boxRef = useRef<HTMLDivElement | null>(null)
  const [spot, setSpot] = useState<Spot>(() => spotFor(anchor))

  // Leaving Zen (this unmounts) closes it, so it never comes back on the
  // next Zen-on or lingers over the regular Home.
  useEffect(() => () => closeZenAddAgent(), [])

  // The picker is for one Home in one Garden: a Garden switch, that Home
  // going away, or the widget's Home picker moving to another Home closes it.
  useEffect(() => {
    if (!open) return
    const gardenMoved = openedIn !== null && gardenId !== null && openedIn !== gardenId
    const homeMoved = pickedNow !== null && pickedNow !== homeId
    if (!home || gardenMoved || homeMoved) closeZenAddAgent()
  }, [open, home, homeId, openedIn, gardenId, pickedNow])

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

  if (!open || !home) return null
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
      <AddAgentPicker home={home} onAdded={noteZenAgentAdded} />
    </div>
  )
}

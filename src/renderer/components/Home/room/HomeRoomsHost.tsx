// prd-home-seamless-0432 item 2 (Z8, Z9, Z32) — Home's remote rooms live
// OUTSIDE the keyed App, so a top-switcher change does not unmount them.
//
// `<App key={hostKey}>` remounts on every window server switch. Before
// 0.43.2 the rooms rendered inside it (`AgentsShell` → `HomeRemoteRooms`),
// so a switch closed every grid, chat-transcript, overlay and pinned-Chat
// socket a room's components own, and the row had to be clicked again.
//
// Now:
//   - `HomeRoomsPortal` renders the rooms through `createPortal` into ONE
//     container element made once per page. `ConnectionGate` mounts it (via
//     the App chunk's `HomeRoomsHost`) as the first child of every branch,
//     so React never remounts it while the gate moves wait → overlay →
//     accept.
//   - `RoomSlot` sits where `<HomeRemoteRooms/>` used to (the Agents shell's
//     main area). On mount it moves the container into itself; on unmount it
//     hands the container to the parking element. Moving a DOM node does not
//     unmount React components, so xterm instances and canvases survive.
//   - Parking (Z9): a `position:fixed`, `visibility:hidden`,
//     `pointer-events:none` element at the slot's last rect, so terminals
//     never refit. Rooms keep `shown`; PageLive does not change, so the shown
//     room's grids stay open. Home's page visibility goes false while parked
//     (`homeRooms.setPageVisible`: only the tier clock starts), and
//     `RoomParkedContext` tells native Browser webviews to hide (Z33).
//
// The drawers stay in App (Z10): they own no socket and refetch over HTTP.

import { useEffect, useLayoutEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { create } from 'zustand'
import { RoomParkedContext } from '@/contexts/RoomParkedContext'
import { AppErrorBoundary } from '@/components/AppErrorBoundary'
import { homeRooms } from '@/stores/home-rooms'
import { usePageViewStore } from '@/stores/page-view'
import { HomeRemoteRooms } from './HomeRemoteRooms'

export interface ParkRect {
  left: number
  top: number
  width: number
  height: number
}

interface RoomParkState {
  /** True while no slot holds the rooms (they sit in the parking element). */
  parked: boolean
  /** The slot's last rect: where the parking element sits. */
  rect: ParkRect | null
}

/** React: is the rooms container parked, and where. */
export const useRoomParkStore = create<RoomParkState>(() => ({ parked: true, rect: null }))

let container: HTMLDivElement | null = null
let parking: HTMLDivElement | null = null
let currentSlot: HTMLElement | null = null

/** The one element every remote room renders into (made on first use). */
export function homeRoomsContainer(): HTMLDivElement {
  if (!container) {
    container = document.createElement('div')
    container.className = 'h-full w-full'
    container.setAttribute('data-home-rooms-container', '')
  }
  return container
}

/** The parking element (made and attached to `body` on first use). */
export function homeRoomsParking(): HTMLDivElement {
  if (!parking) {
    parking = document.createElement('div')
    parking.setAttribute('data-home-rooms-parking', '')
    parking.setAttribute('aria-hidden', 'true')
    Object.assign(parking.style, {
      position: 'fixed',
      visibility: 'hidden',
      pointerEvents: 'none',
      overflow: 'hidden',
      // Under the gate's overlays (they sit well above 0).
      zIndex: '0',
      left: '0px',
      top: '0px',
      width: '0px',
      height: '0px',
    })
  }
  if (!parking.isConnected) document.body.appendChild(parking)
  return parking
}

function placeParking(rect: ParkRect | null): void {
  const p = homeRoomsParking()
  const r = rect ?? { left: 0, top: 0, width: 0, height: 0 }
  p.style.left = `${r.left}px`
  p.style.top = `${r.top}px`
  p.style.width = `${r.width}px`
  p.style.height = `${r.height}px`
}

function rectOf(el: HTMLElement): ParkRect | null {
  const r = el.getBoundingClientRect()
  if (r.width === 0 && r.height === 0) return null
  return { left: r.left, top: r.top, width: r.width, height: r.height }
}

/** Move the container into `slot` (unpark). Returns the release that parks
 *  it again at the slot's last rect, unless another slot took it since. */
export function attachRoomSlot(slot: HTMLElement): () => void {
  const c = homeRoomsContainer()
  currentSlot = slot
  slot.appendChild(c)
  useRoomParkStore.setState({ parked: false, rect: rectOf(slot) ?? useRoomParkStore.getState().rect })
  return () => {
    if (currentSlot !== slot) return
    currentSlot = null
    const rect = rectOf(slot) ?? useRoomParkStore.getState().rect
    placeParking(rect)
    homeRoomsParking().appendChild(c)
    useRoomParkStore.setState({ parked: true, rect })
  }
}

/** Keep the parking rect current while the slot is mounted (a window
 *  resize between the last measure and the switch). */
function noteSlotRect(slot: HTMLElement): void {
  if (currentSlot !== slot) return
  const rect = rectOf(slot)
  if (rect) useRoomParkStore.setState({ rect })
}

/** Where Home's remote rooms appear in the Agents shell's main area. It
 *  takes space only while a remote room is on screen (`visible`). */
export function RoomSlot({ visible }: { visible: boolean }): React.JSX.Element {
  const ref = useRef<HTMLDivElement | null>(null)
  useLayoutEffect(() => {
    const slot = ref.current
    if (!slot) throw new Error('RoomSlot: no element')
    const release = attachRoomSlot(slot)
    let ro: ResizeObserver | null = null
    if (typeof ResizeObserver !== 'undefined') {
      ro = new ResizeObserver(() => noteSlotRect(slot))
      ro.observe(slot)
    }
    return () => {
      ro?.disconnect()
      release()
    }
  }, [])
  return (
    <div
      ref={ref}
      className="h-full w-full"
      style={visible ? undefined : { display: 'none' }}
      data-home-room-slot=""
    />
  )
}

/** The rooms themselves, portalled into the container. Mounted once by the
 *  gate (through App's `HomeRoomsHost`), never inside the keyed App. */
export function HomeRoomsPortal(): React.ReactPortal {
  const parked = useRoomParkStore((s) => s.parked)
  const onHome = usePageViewStore((s) => s.page === 'home')
  useEffect(() => {
    homeRooms.setPageVisible(onHome && !parked)
  }, [onHome, parked])
  return createPortal(
    <AppErrorBoundary>
      <RoomParkedContext.Provider value={parked}>
        <HomeRemoteRooms />
      </RoomParkedContext.Provider>
    </AppErrorBoundary>,
    homeRoomsContainer(),
  )
}

/** Tests only: forget the container and the parking element. */
export function __resetHomeRoomsHostForTests(): void {
  container?.remove()
  parking?.remove()
  container = null
  parking = null
  currentSlot = null
  useRoomParkStore.setState({ parked: true, rect: null })
}

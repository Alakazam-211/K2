// The focused-room pointer (Home M3, prd-home-multi-server-client MS17/MS18).
//
// Window-level input — the native menu, keyboard shortcuts, the Workspace
// Assistant's `workspace:*` ops, Cmd+[ / Cmd+] — acts on ONE room: the
// focused one. Never "whatever the window is on", and never every mounted
// room at once (two rooms each handling Cmd+T would open two tabs).
//
//   - A room's shell registers it while mounted (`RoomProvider` with
//     `shown`). The room shown in the main area takes focus when it mounts.
//   - Pointer-down or focus-in inside an element carrying `data-room-key`
//     focuses that room (one capture listener per window).
//   - When the focused room unmounts, focus moves to the most recently
//     mounted room still shown, or to null.
//   - A window-level action with a null pointer does nothing.
//
// Per window, never persisted.

import { create } from 'zustand'
import type { Room } from '@/stores/room'

interface WindowRoomState {
  /** Rooms mounted and shown in this window, oldest first. */
  shown: Room[]
  focused: Room | null
}

export const useWindowRoomStore = create<WindowRoomState>(() => ({
  shown: [],
  focused: null,
}))

/** The room window-level input acts on, or null (then it does nothing). */
export function focusedRoom(): Room | null {
  return useWindowRoomStore.getState().focused
}

/** Is `room` the focused room? Per-room listeners (one per mounted terminal
 *  area) return early unless it is (MS18). */
export function isFocusedRoom(room: Room): boolean {
  return useWindowRoomStore.getState().focused === room
}

/** Focus `room` (a pointer-down / focus-in inside it). Only a shown room can
 *  take focus: hidden hot rooms (M4) never do. */
export function focusRoom(room: Room): void {
  const s = useWindowRoomStore.getState()
  if (!s.shown.includes(room)) return
  if (s.focused !== room) useWindowRoomStore.setState({ focused: room })
}

/** A room's shell mounted and is shown: register it and give it focus. */
export function roomShown(room: Room): void {
  const s = useWindowRoomStore.getState()
  const shown = s.shown.filter((r) => r !== room)
  shown.push(room)
  useWindowRoomStore.setState({ shown, focused: room })
}

/** A room's shell unmounted (or was hidden): drop it; if it had focus, focus
 *  moves to the most recently shown room still mounted, or null. */
export function roomHidden(room: Room): void {
  const s = useWindowRoomStore.getState()
  if (!s.shown.includes(room)) return
  const shown = s.shown.filter((r) => r !== room)
  const focused = s.focused === room ? (shown[shown.length - 1] ?? null) : s.focused
  useWindowRoomStore.setState({ shown, focused })
}

/** The room whose root element contains `target`, by `data-room-key`. */
export function roomForElement(target: EventTarget | null): Room | null {
  if (!target || typeof (target as Element).closest !== 'function') return null
  const el = (target as Element).closest('[data-room-key]')
  if (!el) return null
  const key = el.getAttribute('data-room-key')
  return useWindowRoomStore.getState().shown.find((r) => r.key === key) ?? null
}

/** One capture listener per window: pointer-down or focus-in inside a room
 *  root focuses that room. Returns the uninstall. */
export function installRoomFocusTracking(doc: Document): () => void {
  const onEvent = (e: Event): void => {
    const room = roomForElement(e.target)
    if (room) focusRoom(room)
  }
  doc.addEventListener('pointerdown', onEvent, true)
  doc.addEventListener('focusin', onEvent, true)
  return () => {
    doc.removeEventListener('pointerdown', onEvent, true)
    doc.removeEventListener('focusin', onEvent, true)
  }
}

export function __resetWindowRoomForTests(): void {
  useWindowRoomStore.setState({ shown: [], focused: null })
}

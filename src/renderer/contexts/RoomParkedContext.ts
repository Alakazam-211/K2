import { createContext, useContext } from 'react'

/**
 * prd-home-seamless-0432 Z9/Z33 — true while Home's remote rooms are
 * parked off screen (their slot in the Agents shell is unmounted, e.g. while
 * a top-switcher change remounts the App). A parked room keeps `shown` and
 * PageLive, so its grids stay open, but its DOM is invisible. Native child
 * webviews (the Browser pane) ignore the DOM, so they read this and hide;
 * otherwise they would draw over the connecting overlay.
 *
 * Default false: everything outside the rooms host is never parked.
 */
export const RoomParkedContext = createContext<boolean>(false)

export function useRoomParked(): boolean {
  return useContext(RoomParkedContext)
}

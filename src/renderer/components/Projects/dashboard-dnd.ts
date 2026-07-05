// Projects V1 P5 (§6.2) — the tiny bridge that lets a MEMBER-ROW drag
// (ProjectNav's drawer, Sidebar's mouse-threshold pattern) land on the
// dashboard grid (ProjectDashboard), which lives in a different
// component subtree.
//
// The INITIATOR (MemberRow) owns the drag lifecycle exactly like
// Sidebar.handleProjectMouseDown: mousedown → 3/5px threshold →
// document mousemove/mouseup. While dragging it streams the pointer
// into this store (the dashboard renders its drop-target markers from
// it); on mouseup it hands the final point to whichever dashboard
// registered a drop handler (none mounted → the drag simply cancels).

import { create } from 'zustand'

export interface MemberDrag {
  workspaceId: string
  x: number
  y: number
}

interface DashboardDndState {
  /** Live member drag, or null. The dashboard subscribes for markers. */
  memberDrag: MemberDrag | null
  setMemberDrag: (drag: MemberDrag | null) => void
}

export const useDashboardDndStore = create<DashboardDndState>((set) => ({
  memberDrag: null,
  setMemberDrag: (memberDrag) => set({ memberDrag }),
}))

type MemberDropHandler = (workspaceId: string, x: number, y: number) => void

let dropHandler: MemberDropHandler | null = null

/** The mounted dashboard registers itself as the drop target; returns
 *  the unregister. Last-mount-wins (only one dashboard renders at a
 *  time — the tab is exclusive). */
export function registerMemberDropHandler(handler: MemberDropHandler): () => void {
  dropHandler = handler
  return () => {
    if (dropHandler === handler) dropHandler = null
  }
}

/** Initiator mouseup → offer the drop to the registered dashboard. */
export function completeMemberDrag(workspaceId: string, x: number, y: number): void {
  dropHandler?.(workspaceId, x, y)
}

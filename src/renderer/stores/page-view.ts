// Projects V1 P4 (prd-projects-v1 §6.0) — which top-level PAGE the app
// shows. The top bar's switcher (Home | Agents | Projects | Tickets |
// Wiki | ⚙) selects one of the page views: 'agents' is today's default workspace
// view (Sidebar + tabs + terminal); 'home' is the SAME shell with the Home
// roster in the sidebar (agents picked from any saved server — Home P1,
// prd-home-v1 H2: "reuses the Agents page shell"); 'projects', 'feedback'
// and 'wiki' render as full-page overlays gated on this store. Settings is a
// store overlay (settingsOpen), not an AppPage — the cog is a tab that opens
// it. Wiki is a page tab too (0.46.0, Rosson 2026-10-16): the tab opens the
// wiki of the workspace on screen (`lib/wiki-open.ts`); the drawer's View
// Wiki and the nav menus open one workspace's wiki directly. Close (Esc or
// the X) returns to the page the wiki was opened from. All of this is
// per-window view state (thin client).
//
// This is the SSOT for "which page" — `useFeedbackStore.isOpen` mirrors
// it (feedback.ts subscribes) so every pre-switcher consumer of that
// flag keeps working.

import { create } from 'zustand'
import { readWikiViewPref, writeWikiViewPref, type WikiView } from '@/lib/wiki-view-pref'

export type AppPage = 'home' | 'agents' | 'projects' | 'feedback' | 'wiki'

/** Pages that show the workspace room shell (top bar, sidebar, tab strip,
 *  panes, drawers). Home and Agents share it; only the sidebar's list
 *  differs. Everything that asks "is the room on screen?" uses this. */
export function isRoomPage(page: AppPage): boolean {
  return page === 'agents' || page === 'home'
}

/** `PageLiveContext` for the window's own (primary) room: true while its
 *  terminals may hold grids. Agents, and Home while a room is shown — never
 *  while Zen covers the window (prd-zen-gardens-v1 G4/G49: Zen is a mode of
 *  the window, so whatever page is under it parks like a hidden tab). */
export function primaryRoomPageLive(page: AppPage, homeRoomShown: boolean, zenShown: boolean): boolean {
  return !zenShown && (page === 'agents' || (page === 'home' && homeRoomShown))
}

/** A page the wiki can return to on close (any page but the wiki). */
export type WikiReturnPage = Exclude<AppPage, 'wiki'>

interface PageViewState {
  page: AppPage
  /** Workspace path whose wiki is open when `page === 'wiki'`. */
  wikiProjectPath: string | null
  /** The page the wiki was opened from; close returns there. */
  wikiReturnPage: WikiReturnPage
  /** The last workspace wiki shown in this window (kept after close): the
   *  Wiki tab's fallback when the view has no workspace of this server. */
  lastWikiPath: string | null
  setPage: (page: AppPage) => void
  /** Open the wiki page for a workspace path (null: no workspace yet).
   *  While the wiki is open it only switches the workspace; the page to
   *  return to is kept. `allOfK2`: open on the server-wide map (the top
   *  bar's Wiki tab); deep links (View Wiki, nav menus) open the workspace. */
  openWiki: (projectPath: string | null, opts?: { allOfK2?: boolean }) => void
  /** The last openWiki asked for All of K2 (WikiPage reads it when it opens). */
  wikiOpenAll: boolean
  /** Close the wiki and return to the page it was opened from. */
  closeWiki: () => void
  /** The Wiki page's view: standard reading view, 2D graph or 3D graph.
   *  Per window and persisted (`lib/wiki-view-pref.ts`); open/close/setPage
   *  never touch it (prd-wiki-3d-view-v1 §6.6). It lives here, not in
   *  WikiPage state, because WikiPage unmounts while Settings is open and
   *  resets its state on close. */
  wikiView: WikiView
  setWikiView: (view: WikiView) => void
}

export const usePageViewStore = create<PageViewState>((set) => ({
  page: 'agents',
  wikiProjectPath: null,
  wikiReturnPage: 'agents',
  lastWikiPath: null,
  wikiOpenAll: false,
  wikiView: readWikiViewPref(),
  setWikiView: (view) => {
    writeWikiViewPref(view)
    set({ wikiView: view })
  },
  setPage: (page) =>
    set((s) => ({
      page,
      // Leaving wiki clears the bound workspace path so a later open
      // does not flash stale data under a different project.
      wikiProjectPath: page === 'wiki' ? s.wikiProjectPath : null,
      wikiReturnPage: page === 'wiki' && s.page !== 'wiki' ? s.page : s.wikiReturnPage,
    })),
  openWiki: (projectPath, opts) =>
    set((s) => ({
      page: 'wiki',
      wikiOpenAll: opts?.allOfK2 === true,
      wikiProjectPath: projectPath,
      wikiReturnPage: s.page === 'wiki' ? s.wikiReturnPage : s.page,
      lastWikiPath: projectPath ?? s.lastWikiPath,
    })),
  closeWiki: () =>
    set((s) => ({
      page: s.page === 'wiki' ? s.wikiReturnPage : s.page,
      wikiProjectPath: null,
      wikiReturnPage: 'agents',
    })),
}))

// Projects V1 P4 (prd-projects-v1 §6.0) — which top-level PAGE the app
// shows. The top bar's switcher (⚙ | Agents | Projects | Tickets)
// selects one of three page views: 'agents' is today's default workspace
// view (Sidebar + tabs + terminal); 'projects' and 'feedback' render as
// full-page overlays gated on this store. Settings is a store overlay
// (settingsOpen), not an AppPage — the cog is a fourth tab that opens
// it. Wiki is opened only from the workspace drawer (no permanent
// PageTabs entry); Esc or selecting a switcher tab returns to agents /
// that page.
//
// This is the SSOT for "which page" — `useFeedbackStore.isOpen` mirrors
// it (feedback.ts subscribes) so every pre-switcher consumer of that
// flag keeps working.

import { create } from 'zustand'

export type AppPage = 'agents' | 'projects' | 'feedback' | 'wiki'

interface PageViewState {
  page: AppPage
  /** Workspace path whose wiki is open when `page === 'wiki'`. */
  wikiProjectPath: string | null
  setPage: (page: AppPage) => void
  /** Open the wiki overlay for a workspace path (from WorkspacePanel). */
  openWiki: (projectPath: string) => void
  /** Close wiki and return to Agents. */
  closeWiki: () => void
}

export const usePageViewStore = create<PageViewState>((set) => ({
  page: 'agents',
  wikiProjectPath: null,
  setPage: (page) =>
    set((s) => ({
      page,
      // Leaving wiki clears the bound workspace path so a later open
      // does not flash stale data under a different project.
      wikiProjectPath: page === 'wiki' ? s.wikiProjectPath : null,
    })),
  openWiki: (projectPath) =>
    set({ page: 'wiki', wikiProjectPath: projectPath }),
  closeWiki: () => set({ page: 'agents', wikiProjectPath: null }),
}))

// Projects V1 P4 (prd-projects-v1 §6.0) — which top-level PAGE the app
// shows. The top bar's 3-tab switcher (Agents | Projects | Feedback)
// selects one of three page views: 'agents' is today's default workspace
// view (Sidebar + tabs + terminal); 'projects' and 'feedback' render as
// full-page overlays gated on this store. This is the SSOT for "which
// page" — `useFeedbackStore.isOpen` mirrors it (feedback.ts subscribes)
// so every pre-switcher consumer of that flag keeps working.

import { create } from 'zustand'

export type AppPage = 'agents' | 'projects' | 'feedback'

interface PageViewState {
  page: AppPage
  setPage: (page: AppPage) => void
}

export const usePageViewStore = create<PageViewState>((set) => ({
  page: 'agents',
  setPage: (page) => set({ page }),
}))

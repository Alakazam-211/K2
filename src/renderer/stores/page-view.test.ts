// Page-view store (§6.0) — the top-bar 3-tab switcher's SSOT, and the
// feedback store's delegation into it: `useFeedbackStore.open/close/
// toggle` were the pre-switcher entry/exit for the Feedback page, so
// they must keep working (behavioral parity) by writing through the
// page store, with `isOpen` mirroring `page === 'feedback'`.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// feedback.ts's boundary deps (feedback.test.ts idiom) — inert here.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/components/Feedback/feedback-api', () => ({
  fetchWaitingCount: vi.fn(async () => 0),
}))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ projects: [] }) },
}))

import { usePageViewStore } from '@/stores/page-view'
import { useFeedbackStore } from '@/stores/feedback'

describe('page-view switcher + feedback delegation', () => {
  beforeEach(() => {
    usePageViewStore.getState().setPage('agents')
  })

  it('defaults to the agents page', () => {
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(useFeedbackStore.getState().isOpen).toBe(false)
  })

  it('feedback open/close route through the page store and mirror isOpen', () => {
    useFeedbackStore.getState().open()
    expect(usePageViewStore.getState().page).toBe('feedback')
    expect(useFeedbackStore.getState().isOpen).toBe(true)

    useFeedbackStore.getState().close()
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(useFeedbackStore.getState().isOpen).toBe(false)
  })

  it('feedback toggle flips between feedback and agents', () => {
    useFeedbackStore.getState().toggle()
    expect(usePageViewStore.getState().page).toBe('feedback')
    useFeedbackStore.getState().toggle()
    expect(usePageViewStore.getState().page).toBe('agents')
  })

  it('selecting the Projects tab closes an open Feedback page', () => {
    useFeedbackStore.getState().open()
    usePageViewStore.getState().setPage('projects')
    expect(useFeedbackStore.getState().isOpen).toBe(false)
    expect(usePageViewStore.getState().page).toBe('projects')
  })
})

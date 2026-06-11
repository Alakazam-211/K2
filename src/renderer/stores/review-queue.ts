import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from './projects'
import { serverSupports } from '@/lib/server-capabilities'
import { subscribeToWorkspaceReviewEvents, type UnsubscribeFn } from './session-events'

interface ReviewDiffFile {
  path: string
  status: string
  additions: number
  deletions: number
}

interface WorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  itemType: string
  folder: string
}

interface ReviewItem {
  agentName: string
  branch: string
  worktreePath: string | null
  workItems: WorkItem[]
  diffSummary: ReviewDiffFile[]
}

export interface GlobalReviewItem extends ReviewItem {
  projectId: string
  projectName: string
  projectColor: string
  projectPath: string
  workspaceId: string | null
}

interface ReviewQueueState {
  isOpen: boolean
  reviews: GlobalReviewItem[]
  pendingCount: number
  loading: boolean

  open: () => void
  close: () => void
  toggle: () => void
  fetchAll: () => Promise<void>
}

export const useReviewQueueStore = create<ReviewQueueState>((set) => ({
  isOpen: false,
  reviews: [],
  pendingCount: 0,
  loading: false,

  open: () => {
    set({ isOpen: true })
    useReviewQueueStore.getState().fetchAll()
  },
  close: () => set({ isOpen: false }),
  toggle: () => {
    const { isOpen } = useReviewQueueStore.getState()
    if (!isOpen) {
      useReviewQueueStore.getState().open()
    } else {
      set({ isOpen: false })
    }
  },

  fetchAll: async () => {
    set({ loading: true })
    const projectsState = useProjectsStore.getState()
    const activeProjectId = projectsState.activeProjectId
    // Only check the active project to avoid hammering git across all repos
    const projects = activeProjectId
      ? projectsState.projects.filter((p) => p.id === activeProjectId)
      : []
    const allReviews: GlobalReviewItem[] = []

    for (const project of projects) {
      if (!project.agentEnabled && (!project.agentMode || project.agentMode === 'off')) continue
      try {
        const reviews = await invoke<ReviewItem[]>('k2so_agents_review_queue', {
          projectPath: project.path,
        })
        for (const review of reviews) {
          // Find matching workspace for jump-to — try branch match, worktree path, or partial branch match
          const ws = project.workspaces.find(
            (w) =>
              w.branch === review.branch ||
              w.worktreePath === review.worktreePath ||
              (w.branch && review.branch && review.branch.includes(w.branch)) ||
              (w.branch && review.branch && w.branch.includes(review.branch))
          )
          allReviews.push({
            ...review,
            projectId: project.id,
            projectName: project.name,
            projectColor: project.color,
            projectPath: project.path,
            workspaceId: ws?.id ?? null,
          })
        }
      } catch {
        // Skip projects with errors
      }
    }

    set({
      reviews: allReviews,
      pendingCount: allReviews.length,
      loading: false,
    })
  },
}))

// 0.39.39 (#675.3) — push-primary with snapshot-on-connect. The 30s
// `fetchAll` poll is replaced by a subscription to the daemon's workspace-
// scoped review broadcasts (`review_queue_changed` / `review_changed`); each
// event re-fetches the queue for the active project. The queue is scoped to
// the active project (see `fetchAll`), so we (re)subscribe on the active
// project's path and swap the subscription when the active project changes.
// Against an OLDER / REMOTE daemon that doesn't emit the broadcasts we KEEP
// the 30s poll fallback.
let pollInterval: ReturnType<typeof setInterval> | null = null
let reviewEventsUnsub: UnsubscribeFn | null = null
let projectStoreUnsub: (() => void) | null = null
let subscribedPath: string | null = null

function activeProjectPath(): string | null {
  const s = useProjectsStore.getState()
  if (!s.activeProjectId) return null
  return s.projects.find((p) => p.id === s.activeProjectId)?.path ?? null
}

function resubscribeReviewEvents(): void {
  const path = activeProjectPath()
  if (path === subscribedPath) return
  if (reviewEventsUnsub) {
    reviewEventsUnsub()
    reviewEventsUnsub = null
  }
  subscribedPath = path
  if (!path) return
  reviewEventsUnsub = subscribeToWorkspaceReviewEvents(path, {
    onReviewQueueChanged: () => void useReviewQueueStore.getState().fetchAll(),
    onReviewChanged: () => void useReviewQueueStore.getState().fetchAll(),
    // Re-snapshot on (re)connect to backfill anything missed during a drop.
    onHello: () => void useReviewQueueStore.getState().fetchAll(),
  })
}

export function startReviewQueuePolling(): void {
  if (pollInterval || reviewEventsUnsub || projectStoreUnsub) return
  // Initial snapshot of current truth.
  useReviewQueueStore.getState().fetchAll()

  if (serverSupports('daemon-broadcasts')) {
    resubscribeReviewEvents()
    // Swap the path-scoped subscription whenever the active project changes.
    projectStoreUnsub = useProjectsStore.subscribe((state, prev) => {
      if (state.activeProjectId !== prev.activeProjectId) resubscribeReviewEvents()
    })
  } else {
    pollInterval = setInterval(() => {
      useReviewQueueStore.getState().fetchAll()
    }, 30_000)
  }
}

export function stopReviewQueuePolling(): void {
  if (pollInterval) {
    clearInterval(pollInterval)
    pollInterval = null
  }
  if (reviewEventsUnsub) {
    reviewEventsUnsub()
    reviewEventsUnsub = null
  }
  if (projectStoreUnsub) {
    projectStoreUnsub()
    projectStoreUnsub = null
  }
  subscribedPath = null
}

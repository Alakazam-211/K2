import { useEffect, useRef, useState, useCallback, useMemo } from 'react'
import { useReviewQueueStore, type GlobalReviewItem } from '@/stores/review-queue'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useSidebarStore } from '@/stores/sidebar'
import { usePanelsStore } from '@/stores/panels'

export default function ReviewQueueModal(): React.JSX.Element | null {
  const isOpen = useReviewQueueStore((s) => s.isOpen)
  const close = useReviewQueueStore((s) => s.close)
  const reviews = useReviewQueueStore((s) => s.reviews)
  const loading = useReviewQueueStore((s) => s.loading)

  const setActiveProject = useProjectsStore((s) => s.setActiveProject)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)
  const projects = useProjectsStore((s) => s.projects)
  const expand = useSidebarStore((s) => s.expand)

  const [query, setQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  // Reset on open
  useEffect(() => {
    if (isOpen) {
      setQuery('')
      setSelectedIndex(0)
      requestAnimationFrame(() => inputRef.current?.focus())
    }
  }, [isOpen])

  // Filter results
  const filtered = useMemo(() => {
    if (!query.trim()) return reviews
    const q = query.toLowerCase()
    return reviews.filter(
      (r) =>
        r.projectName.toLowerCase().includes(q) ||
        r.agentName.toLowerCase().includes(q) ||
        r.branch.toLowerCase().includes(q)
    )
  }, [reviews, query])

  // Clamp index
  useEffect(() => {
    setSelectedIndex((prev) => Math.min(prev, Math.max(0, filtered.length - 1)))
  }, [filtered.length])

  // Jump to review
  const jumpToReview = useCallback(
    (review: GlobalReviewItem) => {
      // Set active project
      setActiveProject(review.projectId)

      // Set active workspace to the worktree if available
      if (review.workspaceId) {
        setActiveWorkspace(review.projectId, review.workspaceId)
      } else {
        const project = projects.find((p) => p.id === review.projectId)
        if (project?.workspaces[0]) {
          setActiveWorkspace(project.id, project.workspaces[0].id)
        }
      }

      // Set focus group if applicable
      const project = projects.find((p) => p.id === review.projectId)
      if (project?.focusGroupId) {
        // autoActivate: false — the review's workspace was activated
        // explicitly above; the nested first-project activation would
        // double-switch and race the pinned-tab resolution.
        useFocusGroupsStore.getState().setActiveFocusGroup(project.focusGroupId, { autoActivate: false })
      }

      // Open Reviews panel
      const panels = usePanelsStore.getState()
      if (panels.rightPanelTabs.includes('reviews')) {
        panels.setRightPanelActiveTab('reviews')
        if (!panels.rightPanelOpen) panels.toggleRightPanel()
      }

      expand()
      close()
    },
    [projects, setActiveProject, setActiveWorkspace, expand, close]
  )

  // Keyboard
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
        return
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSelectedIndex((prev) => Math.min(prev + 1, filtered.length - 1))
        return
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSelectedIndex((prev) => Math.max(prev - 1, 0))
        return
      }
      if (e.key === 'Enter') {
        e.preventDefault()
        if (filtered[selectedIndex]) {
          jumpToReview(filtered[selectedIndex])
        }
        return
      }
    },
    [close, filtered, selectedIndex, jumpToReview]
  )

  // Scroll into view
  useEffect(() => {
    const list = listRef.current
    if (!list) return
    const item = list.children[selectedIndex] as HTMLElement | undefined
    if (item) item.scrollIntoView({ block: 'nearest' })
  }, [selectedIndex])

  if (!isOpen) return null

  // Group reviews by project
  const grouped = new Map<string, GlobalReviewItem[]>()
  for (const review of filtered) {
    const key = review.projectId
    if (!grouped.has(key)) grouped.set(key, [])
    grouped.get(key)!.push(review)
  }

  let globalIndex = 0

  return (
    <div
      className="fixed inset-0 z-[9999] flex items-start justify-center pt-[15vh] no-drag"
      style={{ background: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(8px)' }}
      onClick={(e) => {
        if (e.target === e.currentTarget) close()
      }}
      onKeyDown={handleKeyDown}
    >
      <div
        className="w-[560px] max-h-[60vh] flex flex-col overflow-hidden border border-[var(--color-border)]"
        style={{ background: 'var(--color-bg-surface)', boxShadow: '0 24px 48px rgba(0, 0, 0, 0.5)' }}
      >
        {/* Search input */}
        <div className="flex items-center border-b border-[var(--color-border)] px-4 py-3">
          <svg
            width="14"
            height="14"
            viewBox="0 0 16 16"
            fill="none"
            className="flex-shrink-0 mr-3"
            style={{ color: 'var(--color-text-muted)' }}
          >
            <path
              d="M11.5 7a4.5 4.5 0 1 1-9 0 4.5 4.5 0 0 1 9 0ZM10.643 11.357a6 6 0 1 1 .714-.714l3.85 3.85a.5.5 0 0 1-.707.707l-3.857-3.843Z"
              fill="currentColor"
            />
          </svg>
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value)
              setSelectedIndex(0)
            }}
            placeholder="Search reviews by project, agent, or branch..."
            spellCheck={false}
            autoComplete="off"
            className="flex-1 bg-transparent text-sm text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none"
            style={{ fontFamily: 'inherit' }}
          />
          <kbd
            className="ml-2 text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)]"
            style={{ background: 'var(--color-bg)' }}
          >
            ESC
          </kbd>
        </div>

        {/* Results */}
        <div ref={listRef} className="overflow-y-auto flex-1" style={{ scrollbarWidth: 'thin' }}>
          {loading && filtered.length === 0 && (
            <div className="px-4 py-8 text-center text-xs text-[var(--color-text-muted)]">
              Loading reviews...
            </div>
          )}

          {!loading && filtered.length === 0 && (
            <div className="px-4 py-8 text-center">
              <div className="text-green-400 text-lg mb-2">&#10003;</div>
              <div className="text-xs text-[var(--color-text-muted)]">
                No pending reviews
              </div>
            </div>
          )}

          {Array.from(grouped.entries()).map(([projectId, projectReviews]) => {
            const project = projectReviews[0]
            return (
              <div key={projectId}>
                {/* Project header */}
                <div className="px-4 pt-3 pb-1 text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] flex items-center gap-2">
                  <span
                    className="w-2 h-2 rounded-full flex-shrink-0"
                    style={{ backgroundColor: project.projectColor }}
                  />
                  {project.projectName}
                </div>

                {/* Review items */}
                {projectReviews.map((review) => {
                  const idx = globalIndex++
                  const isSelected = idx === selectedIndex
                  const totalAdditions = review.diffSummary.reduce((s, f) => s + f.additions, 0)
                  const totalDeletions = review.diffSummary.reduce((s, f) => s + f.deletions, 0)

                  return (
                    <div
                      key={`${review.agentName}-${review.branch}`}
                      className="flex items-center gap-3 px-4 py-2 cursor-pointer"
                      style={{
                        background: isSelected ? 'var(--color-bg-elevated)' : 'transparent',
                      }}
                      onClick={() => jumpToReview(review)}
                      onMouseEnter={() => setSelectedIndex(idx)}
                    >
                      {/* Agent name */}
                      <div className="flex-1 min-w-0">
                        <div className="text-xs text-[var(--color-text-primary)] truncate">
                          {review.agentName}
                          {review.branch && (
                            <span className="text-[var(--color-text-muted)] ml-1.5 font-normal">
                              {review.branch}
                            </span>
                          )}
                        </div>
                        <div className="text-[10px] text-[var(--color-text-muted)] truncate">
                          {review.workItems.map((w) => w.title).join(', ')}
                        </div>
                      </div>

                      {/* Diff stats */}
                      <div className="flex items-center gap-1.5 text-[10px] flex-shrink-0">
                        <span className="text-[var(--color-text-muted)]">
                          {review.diffSummary.length} files
                        </span>
                        {totalAdditions > 0 && (
                          <span className="text-green-400">+{totalAdditions}</span>
                        )}
                        {totalDeletions > 0 && (
                          <span className="text-red-400">-{totalDeletions}</span>
                        )}
                      </div>

                      {/* Item count */}
                      <span className="text-[10px] text-green-400 flex-shrink-0">
                        {review.workItems.length} done
                      </span>
                    </div>
                  )
                })}
              </div>
            )
          })}
        </div>

        {/* Footer */}
        <div className="border-t border-[var(--color-border)] px-4 py-2 flex items-center justify-between text-[10px] text-[var(--color-text-muted)]">
          <span>{filtered.length} review{filtered.length !== 1 ? 's' : ''} pending</span>
          <div className="flex items-center gap-3">
            <span>
              <kbd className="px-1 py-0.5 border border-[var(--color-border)]" style={{ background: 'var(--color-bg)' }}>↑↓</kbd>
              {' '}navigate
            </span>
            <span>
              <kbd className="px-1 py-0.5 border border-[var(--color-border)]" style={{ background: 'var(--color-bg)' }}>↵</kbd>
              {' '}jump
            </span>
          </div>
        </div>
      </div>
    </div>
  )
}

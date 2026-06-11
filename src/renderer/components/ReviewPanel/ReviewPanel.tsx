import { useState, useEffect, useCallback, useMemo, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { terminalGetGrid } from '@/lib/terminal-daemon'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { serverSupports } from '@/lib/server-capabilities'
import { subscribeToWorkspaceReviewEvents } from '@/stores/session-events'

// ── Types ───────────────────────────────────────────────────────────

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

interface ChatSession {
  sessionId: string
  project: string
  title: string
  timestamp: number
  provider: string
  messageCount: number
  originBranch: string | null
}

interface ChecklistItem {
  text: string
  checked: boolean
}

// ── Custom Checkbox ─────────────────────────────────────────────────

function ReviewCheckbox({ checked, onChange }: { checked: boolean; onChange: () => void }): React.JSX.Element {
  return (
    <button
      onClick={(e) => { e.stopPropagation(); onChange() }}
      className="w-3.5 h-3.5 flex-shrink-0 mt-0.5 flex items-center justify-center border transition-colors no-drag cursor-pointer"
      style={{
        backgroundColor: checked ? 'var(--color-accent)' : 'transparent',
        borderColor: checked ? 'var(--color-accent)' : 'rgba(255,255,255,0.25)',
      }}
    >
      {checked && (
        <svg className="w-2.5 h-2.5 text-white" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={3.5} strokeLinecap="round" strokeLinejoin="round">
          <polyline points="20 6 9 17 4 12" />
        </svg>
      )}
    </button>
  )
}

// ── Acceptance Criteria Parser ──────────────────────────────────────

function extractAcceptanceCriteria(workItems: WorkItem[], projectPath: string): Promise<ChecklistItem[]> {
  // Read the actual work item files to extract acceptance criteria
  return Promise.all(
    workItems.map(async (item) => {
      try {
        // Try reading the done file content via the filesystem
        const folders = ['done', 'active', 'inbox']
        for (const folder of folders) {
          try {
            const r = await daemonCliGet<{ content: string }>('fs/read-file', {
              path: `${projectPath}/.k2so/agents/${item.assignedBy !== 'user' ? item.assignedBy : 'default'}/work/${folder}/${item.filename}`,
            })
            return parseCriteria(r.content)
          } catch {
            continue
          }
        }
        return []
      } catch {
        return []
      }
    })
  ).then((arrays) => arrays.flat())
}

function parseCriteria(content: string): ChecklistItem[] {
  const items: ChecklistItem[] = []
  // Find ## Acceptance Criteria section
  const match = content.match(/##\s*Acceptance Criteria\s*\n([\s\S]*?)(?=\n##|\n---|\Z|$)/)
  if (!match) return items

  const section = match[1]
  // Parse bullet points as criteria
  for (const line of section.split('\n')) {
    const trimmed = line.trim()
    if (trimmed.startsWith('- [x]') || trimmed.startsWith('- [X]')) {
      items.push({ text: trimmed.slice(5).trim(), checked: true })
    } else if (trimmed.startsWith('- [ ]')) {
      items.push({ text: trimmed.slice(5).trim(), checked: false })
    } else if (trimmed.startsWith('- ') || trimmed.startsWith('* ')) {
      items.push({ text: trimmed.slice(2).trim(), checked: false })
    }
  }
  return items
}

// ── Main Component ──────────────────────────────────────────────────

export default function ReviewPanel(): React.JSX.Element {
  const [reviews, setReviews] = useState<ReviewItem[]>([])
  const [loading, setLoading] = useState(true)
  const [feedbackAgent, setFeedbackAgent] = useState<string | null>(null)
  const [feedbackText, setFeedbackText] = useState('')
  const [acting, setActing] = useState(false)
  const [chats, setChats] = useState<ChatSession[]>([])
  const [criteria, setCriteria] = useState<Map<string, ChecklistItem[]>>(new Map())
  const [previewRunning, setPreviewRunning] = useState<Map<string, string | null>>(new Map()) // agentName → URL or null (pending)
  const previewPollRefs = useRef<Map<string, ReturnType<typeof setInterval>>>(new Map())
  const [checklistItems, setChecklistItems] = useState<Map<string, Array<{ text: string; checked: boolean; section: string }>>>(new Map()) // agentName → items from file

  const agenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)

  const activeProject = projects.find((p) => p.id === activeProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)
  const workspacePath = activeWorkspace?.worktreePath ?? activeProject?.path
  const currentBranch = activeWorkspace?.branch

  // Scope reviews to the active worktree branch
  const scopedReviews = useMemo(() => {
    if (!currentBranch || currentBranch === 'main' || currentBranch === 'master') {
      // On main — show all reviews
      return reviews
    }
    // In a worktree — show only reviews matching this branch
    return reviews.filter(
      (r) => r.branch === currentBranch || r.worktreePath === workspacePath
    )
  }, [reviews, currentBranch, workspacePath])

  const fetchReviews = useCallback(async () => {
    if (!activeProject) {
      setReviews([])
      setLoading(false)
      return
    }
    try {
      const result = await invoke<ReviewItem[]>('k2so_agents_review_queue', {
        projectPath: activeProject.path,
      })
      setReviews(result)

      // Extract acceptance criteria for each review
      const criteriaMap = new Map<string, ChecklistItem[]>()
      for (const review of result) {
        const items = await extractAcceptanceCriteria(review.workItems, activeProject.path)
        if (items.length > 0) {
          criteriaMap.set(review.agentName, items)
        }
      }
      setCriteria(criteriaMap)
    } catch {
      setReviews([])
    } finally {
      setLoading(false)
    }
  }, [activeProject])

  // Fetch associated chat sessions
  const fetchChats = useCallback(async () => {
    if (!workspacePath) return
    try {
      const sessions = await daemonCliGet<ChatSession[]>('chat/list', {
        project_path: workspacePath,
      })
      setChats(sessions)
    } catch {
      setChats([])
    }
  }, [workspacePath])

  // 0.39.39 (#675.4 + #677) — push-primary with snapshot-on-connect. The 15s
  // {fetchReviews; fetchChats} poll is replaced by a subscription to the
  // daemon's workspace-scoped review broadcasts. Each `review_changed` /
  // `review_queue_changed` re-fetches reviews + chats; because `review_changed`
  // fires on every checklist write/toggle/init, the re-fetch updates `reviews`,
  // which re-runs the checklist-read effect below (~232-242) — fixing #677's
  // stale review-checklist cache. On (re)connect (`onHello`) we re-snapshot.
  // Against an OLDER / REMOTE daemon without broadcasts we KEEP the 15s poll.
  const reviewSubPath = activeProject?.path ?? null
  const broadcastsSupported = serverSupports('daemon-broadcasts')
  useEffect(() => {
    fetchReviews()
    fetchChats()

    let interval: ReturnType<typeof setInterval> | null = null
    let unsubReviewEvents: (() => void) | null = null

    if (broadcastsSupported && reviewSubPath) {
      const refetch = (): void => { fetchReviews(); fetchChats() }
      unsubReviewEvents = subscribeToWorkspaceReviewEvents(reviewSubPath, {
        onReviewChanged: refetch,
        onReviewQueueChanged: refetch,
        onHello: refetch,
      })
    } else {
      interval = setInterval(() => { fetchReviews(); fetchChats() }, 15_000)
    }

    // Capture the preview-poll map ref for cleanup (it's mutated elsewhere).
    const previewPolls = previewPollRefs.current
    return () => {
      if (interval) clearInterval(interval)
      if (unsubReviewEvents) unsubReviewEvents()
      // Clean up preview polls
      for (const pollId of previewPolls.values()) {
        clearInterval(pollId)
      }
      previewPolls.clear()
    }
  }, [fetchReviews, fetchChats, broadcastsSupported, reviewSubPath])

  // Load or initialize checklist files for each review
  useEffect(() => {
    for (const review of reviews) {
      const path = review.worktreePath ?? activeProject?.path
      if (!path) continue

      // Build initial items from work items + extracted criteria
      const criteriaItems = criteria.get(review.agentName) ?? []
      const initialItems = [
        ...review.workItems.map((w) => ({ text: w.title || w.filename, checked: false, section: 'verify' })),
        ...criteriaItems.map((c) => ({ text: c.text, checked: false, section: 'criteria' })),
      ]

      // Init the file (only writes if it doesn't exist)
      daemonCliPost('review-checklist/init', {
        workspace_path: path,
        items: initialItems,
        agent_name: review.agentName,
        branch: review.branch,
      }).catch(() => {})

      // Read current state from file
      daemonCliGet<Array<{ text: string; checked: boolean; section: string }>>('review-checklist/read', {
        workspace_path: path,
      }).then((items) => {
        setChecklistItems((prev) => {
          const next = new Map(prev)
          next.set(review.agentName, items)
          return next
        })
      }).catch(() => {})
    }
  }, [reviews, criteria, activeProject?.path])

  // Match chat sessions to review items by branch
  const getChatsForReview = useCallback((review: ReviewItem): ChatSession[] => {
    if (!review.branch) return []
    return chats.filter(
      (c) => c.originBranch === review.branch || c.project.includes(review.branch)
    ).slice(0, 5) // max 5 recent chats
  }, [chats])

  const handleResumeChat = useCallback((session: ChatSession) => {
    if (!workspacePath) return
    const tabsStore = useTabsStore.getState()
    const command = session.provider === 'cursor' ? 'cursor-agent' : 'claude'
    // Include preset flags (e.g. --dangerously-skip-permissions) from user's agent preset
    const presetArgs: string[] = []
    const preset = usePresetsStore.getState().presets.find((p) => p.command.split(/\s+/)[0] === command && p.enabled)
    if (preset) {
      const parts = preset.command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) || []
      const cleaned = parts.map((p: string) => p.replace(/^["']|["']$/g, ''))
      presetArgs.push(...cleaned.slice(1).filter((a: string) => a !== '--resume'))
    }
    tabsStore.addTab(workspacePath, {
      title: `Resume: ${session.title.slice(0, 30)}`,
      command,
      args: [...presetArgs, '--resume', session.sessionId],
    })
  }, [workspacePath])

  const handleStartPreview = useCallback(async (review: ReviewItem) => {
    const path = review.worktreePath ?? workspacePath
    if (!path) return

    // Find the most recent Claude chat session for this review
    const reviewSessions = getChatsForReview(review)
    const claudeSession = reviewSessions.find((s) => s.provider === 'claude')

    const tabsStore = useTabsStore.getState()
    const prompt = 'Launch the dev server for this project on localhost using an available port. If the default port is taken, pick another open one. Once it is running, tell me the full URL.'

    let tabId: string
    if (claudeSession) {
      tabId = tabsStore.addTab(path, {
        title: `Preview: ${review.branch || review.agentName}`,
        command: 'claude',
        args: ['--dangerously-skip-permissions', '--resume', claudeSession.sessionId],
      })
    } else {
      tabId = tabsStore.addTab(path, {
        title: `Preview: ${review.branch || review.agentName}`,
        command: 'claude',
        args: ['--dangerously-skip-permissions', prompt],
      })
    }

    setPreviewRunning((prev) => new Map([...prev, [review.agentName, null]]))

    // Get the terminal ID from the newly created tab
    const tab = tabsStore.tabs.find((t) => t.id === tabId)
    const terminalId = typeof tab?.mosaicTree === 'string' ? tab.mosaicTree : null

    // Poll the terminal grid for a localhost URL
    if (terminalId) {
      // Clear any existing poll for this agent
      const existingPoll = previewPollRefs.current.get(review.agentName)
      if (existingPoll) clearInterval(existingPoll)

      const urlPattern = /https?:\/\/(?:localhost|127\.0\.0\.1):\d+/
      let attempts = 0
      const maxAttempts = 30 // 30 * 2s = 60s

      const pollId = setInterval(async () => {
        attempts++
        try {
          const grid = await terminalGetGrid<{ lines: Array<{ text: string }> }>(terminalId)
          const allText = grid.lines.map((l) => l.text).join('\n')
          const match = allText.match(urlPattern)
          if (match) {
            setPreviewRunning((prev) => new Map([...prev, [review.agentName, match[0]]]))
            clearInterval(pollId)
            previewPollRefs.current.delete(review.agentName)
          }
        } catch {
          // Terminal might not be ready yet
        }
        if (attempts >= maxAttempts) {
          clearInterval(pollId)
          previewPollRefs.current.delete(review.agentName)
          // Reset to allow retry
          setPreviewRunning((prev) => {
            if (prev.get(review.agentName) === null) {
              const next = new Map(prev)
              next.delete(review.agentName)
              return next
            }
            return prev
          })
        }
      }, 2000)

      previewPollRefs.current.set(review.agentName, pollId)
    }
  }, [workspacePath, getChatsForReview])

  const handleApprove = useCallback(async (review: ReviewItem) => {
    if (!activeProject || !review.branch) return
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Approve & Merge "${review.branch}"?`,
      message: `This will merge ${review.diffSummary.length} changed files from agent "${review.agentName}" into main.`,
      confirmLabel: 'Merge',
    })
    if (!confirmed) return

    setActing(true)
    try {
      await daemonCliGet('review/approve', {
        project: activeProject.path,
        branch: review.branch,
        agent: review.agentName,
      })
      await fetchReviews()
    } catch (e) {
      console.error('[review] Approve failed:', e)
    } finally {
      setActing(false)
    }
  }, [activeProject, fetchReviews])

  const handleReject = useCallback(async (review: ReviewItem) => {
    if (!activeProject) return
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Reject "${review.agentName}"'s work?`,
      message: 'Done items will be moved back to the agent\'s inbox.',
      confirmLabel: 'Reject',
      destructive: true,
    })
    if (!confirmed) return

    setActing(true)
    try {
      await daemonCliGet('review/reject', {
        project: activeProject.path,
        agent: review.agentName,
        reason: 'Work rejected by reviewer.',
      })
      await fetchReviews()
    } catch (e) {
      console.error('[review] Reject failed:', e)
    } finally {
      setActing(false)
    }
  }, [activeProject, fetchReviews])

  const handleRequestChanges = useCallback(async () => {
    if (!activeProject || !feedbackAgent || !feedbackText.trim()) return
    setActing(true)
    try {
      await daemonCliGet('review/feedback', {
        project: activeProject.path,
        agent: feedbackAgent,
        feedback: feedbackText.trim(),
      })
      setFeedbackAgent(null)
      setFeedbackText('')
      await fetchReviews()
    } catch (e) {
      console.error('[review] Request changes failed:', e)
    } finally {
      setActing(false)
    }
  }, [activeProject, feedbackAgent, feedbackText, fetchReviews])

  const toggleChecklistItem = useCallback((review: ReviewItem, index: number) => {
    const path = review.worktreePath ?? activeProject?.path
    if (!path) return

    // Optimistic update
    setChecklistItems((prev) => {
      const next = new Map(prev)
      const items = [...(next.get(review.agentName) ?? [])]
      if (items[index]) {
        items[index] = { ...items[index], checked: !items[index].checked }
        next.set(review.agentName, items)
      }
      return next
    })

    // Persist to file
    daemonCliPost<Array<{ text: string; checked: boolean; section: string }>>('review-checklist/toggle', {
      workspace_path: path,
      index,
      agent_name: review.agentName,
      branch: review.branch,
    }).then((items) => {
      setChecklistItems((prev) => {
        const next = new Map(prev)
        next.set(review.agentName, items)
        return next
      })
    }).catch(console.error)
  }, [activeProject?.path])

  // ── Render ──────────────────────────────────────────────────────────

  if (!agenticEnabled) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-[10px] text-[var(--color-text-muted)]">Agentic Systems is off</p>
      </div>
    )
  }

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">No workspace selected</p>
      </div>
    )
  }

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">Loading reviews...</p>
      </div>
    )
  }

  if (scopedReviews.length === 0) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">
          {currentBranch && currentBranch !== 'main'
            ? `No reviews for branch "${currentBranch}"`
            : 'No agent work awaiting review'}
        </p>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto">
      {/* Scope indicator */}
      {currentBranch && currentBranch !== 'main' && currentBranch !== 'master' && (
        <div className="px-3 py-1.5 border-b border-[var(--color-border)] bg-[var(--color-bg-elevated)]">
          <span className="text-[10px] text-[var(--color-text-muted)]">
            Showing reviews for <span className="text-[var(--color-accent)]">{currentBranch}</span>
          </span>
        </div>
      )}

      {scopedReviews.map((review) => {
        const reviewChats = getChatsForReview(review)
        const fileChecklist = checklistItems.get(review.agentName) ?? []
        const isPreviewUp = previewRunning.has(review.agentName)
        const previewUrl = previewRunning.get(review.agentName) ?? null
        const totalAdditions = review.diffSummary.reduce((sum, f) => sum + f.additions, 0)
        const totalDeletions = review.diffSummary.reduce((sum, f) => sum + f.deletions, 0)

        return (
          <div key={review.agentName} className="p-4 flex flex-col gap-4">

            {/* ── Header: agent name + branch ── */}
            <div>
              <div className="text-sm font-medium text-[var(--color-text-primary)]">
                {review.agentName}
              </div>
              {review.branch && (
                <div className="text-[11px] font-mono text-[var(--color-text-muted)] mt-0.5 truncate" title={review.branch}>
                  {review.branch}
                </div>
              )}
            </div>

            {/* ── Stats row ── */}
            <div className="flex items-center gap-3 text-[10px] text-[var(--color-text-muted)]">
              {review.diffSummary.length > 0 && (
                <span>{review.diffSummary.length} file{review.diffSummary.length !== 1 ? 's' : ''}</span>
              )}
              {totalAdditions > 0 && <span className="text-green-400 font-mono">+{totalAdditions}</span>}
              {totalDeletions > 0 && <span className="text-red-400 font-mono">-{totalDeletions}</span>}
              <span className="ml-auto text-green-400">{review.workItems.length} completed</span>
            </div>

            {/* ── Verify Features (file-backed checklist) ── */}
            {fileChecklist.length > 0 && (() => {
              const checkedCount = fileChecklist.filter((i) => i.checked).length
              const totalItems = fileChecklist.length
              return (
                <div>
                  <div className="flex items-center justify-between mb-1.5">
                    <div className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider">
                      Verify Features
                    </div>
                    <span className={`text-[10px] font-mono ${checkedCount === totalItems ? 'text-green-400' : 'text-[var(--color-text-muted)]'}`}>
                      {checkedCount}/{totalItems}
                    </span>
                  </div>
                  <div className="space-y-0.5">
                    {fileChecklist.map((item, i) => (
                      <div
                        key={i}
                        className="flex items-start gap-2 py-0.5 text-[11px] text-[var(--color-text-secondary)] cursor-pointer hover:text-[var(--color-text-primary)]"
                        onClick={() => toggleChecklistItem(review, i)}
                      >
                        <ReviewCheckbox checked={item.checked} onChange={() => toggleChecklistItem(review, i)} />
                        <span className={item.checked ? 'line-through opacity-50' : ''}>
                          {item.text}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              )
            })()}

            {/* ── Preview ── */}
            <div>
              <div className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1.5">
                Preview
              </div>
              {!isPreviewUp ? (
                <button
                  onClick={() => handleStartPreview(review)}
                  className="w-full flex items-center justify-center gap-1.5 py-1.5 text-[11px] font-medium text-white bg-[var(--color-accent)] hover:opacity-90 transition-colors no-drag cursor-pointer"
                >
                  <svg className="w-3 h-3" viewBox="0 0 24 24" fill="currentColor" stroke="none">
                    <polygon points="6,3 20,12 6,21" />
                  </svg>
                  Start Preview
                </button>
              ) : (
                <div
                  className="flex items-center gap-2 py-1.5 px-2 bg-green-500/10 border border-green-500/20 cursor-pointer"
                  onClick={() => {
                    setPreviewRunning((prev) => {
                      const next = new Map(prev)
                      next.delete(review.agentName)
                      return next
                    })
                  }}
                  title="Click to dismiss and retry"
                >
                  <span className="w-1.5 h-1.5 rounded-full bg-green-400 animate-pulse flex-shrink-0" />
                  {previewUrl ? (
                    <span
                      className="text-[11px] text-green-400 font-mono hover:underline truncate cursor-pointer"
                      onClick={(e) => {
                        e.stopPropagation()
                        daemonCliPost('fs/open-external', { target: previewUrl! }).catch(console.warn)
                      }}
                    >
                      {previewUrl}
                    </span>
                  ) : (
                    <div>
                      <span className="text-[11px] text-green-400">Launching...</span>
                      <div className="text-[9px] text-[var(--color-text-muted)]">click to dismiss</div>
                    </div>
                  )}
                </div>
              )}
            </div>

            {/* ── Agent Chat ── */}
            <div>
              <div className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1.5">
                Agent Chat
              </div>
              {reviewChats.length > 0 ? (
                <div>
                  <div className="space-y-1 mb-1.5">
                    {reviewChats.map((chat) => (
                      <div
                        key={chat.sessionId}
                        className="flex items-center gap-2 py-1 px-2 bg-white/[0.03]"
                      >
                        <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-muted)]" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
                        </svg>
                        <span className="text-[10px] text-[var(--color-text-secondary)] truncate flex-1">
                          {chat.title}
                        </span>
                      </div>
                    ))}
                  </div>
                  <button
                    onClick={() => handleResumeChat(reviewChats[0])}
                    className="w-full flex items-center justify-center gap-1.5 py-1.5 text-[11px] font-medium bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
                  >
                    <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
                    </svg>
                    Resume Chat
                  </button>
                </div>
              ) : (
                <button
                  onClick={() => {
                    const path = review.worktreePath ?? workspacePath
                    if (!path) return
                    const tabsStore = useTabsStore.getState()
                    tabsStore.addTab(path, {
                      title: `Chat: ${review.branch || review.agentName}`,
                      command: 'claude',
                      args: ['--dangerously-skip-permissions'],
                    })
                  }}
                  className="w-full flex items-center justify-center gap-1.5 py-1.5 text-[11px] font-medium bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
                >
                  <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
                  </svg>
                  Open Claude in this branch
                </button>
              )}
            </div>

            {/* ── Feedback form ── */}
            {feedbackAgent === review.agentName && (
              <div className="space-y-1.5">
                <textarea
                  className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 resize-none outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
                  placeholder="Describe what needs to change..."
                  rows={3}
                  value={feedbackText}
                  onChange={(e) => setFeedbackText(e.target.value)}
                  autoFocus
                />
                <div className="flex gap-1.5">
                  <button
                    onClick={handleRequestChanges}
                    disabled={acting || !feedbackText.trim()}
                    className="flex-1 py-1 text-[10px] font-medium bg-yellow-600 text-white hover:bg-yellow-500 transition-colors no-drag cursor-pointer disabled:opacity-50"
                  >
                    Send Feedback
                  </button>
                  <button
                    onClick={() => { setFeedbackAgent(null); setFeedbackText('') }}
                    className="px-3 py-1 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}

            {/* ── Actions ── */}
            <div className="flex flex-col gap-1.5 pt-2 border-t border-[var(--color-border)]">
              <button
                onClick={() => handleApprove(review)}
                disabled={acting || !review.branch}
                className="w-full py-1.5 text-[11px] font-medium bg-green-600 text-white hover:bg-green-500 transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                Approve & Merge
              </button>
              <div className="flex gap-1.5">
                <button
                  onClick={() => setFeedbackAgent(review.agentName)}
                  disabled={acting}
                  className="flex-1 py-1.5 text-[11px] font-medium bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
                >
                  Request Changes
                </button>
                <button
                  onClick={() => handleReject(review)}
                  disabled={acting}
                  className="flex-1 py-1.5 text-[11px] text-red-400 border border-red-500/30 hover:bg-red-500/10 transition-colors no-drag cursor-pointer disabled:opacity-50"
                >
                  Reject
                </button>
              </div>
            </div>

            {/* Separator between reviews */}
            <div className="border-b border-[var(--color-border)]" />
          </div>
        )
      })}
    </div>
  )
}

import { useState, useEffect, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import { terminalExists } from '@/lib/terminal-daemon'
import { useTabsStore } from '@/stores/tabs'
import { useProjectsStore } from '@/stores/projects'
import { addNavWorktree } from '@/components/Sidebar/Sidebar'
import { TerminalPane } from '@/terminal-v2/TerminalPane'
import { agentChatId } from '@/lib/terminal-id'
import { AgentInboxPane } from './AgentInboxPane'
import { AgentChatPane } from './AgentChatPane'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'

interface AgentPaneProps {
  agentName: string
  projectPath: string
  /** Which surface to render. Pinned tabs created post-0.36.0 set this
   *  explicitly. `undefined` falls back to 'inbox' for backwards compat
   *  with rows serialized before the split. */
  section?: 'inbox' | 'chat'
  /** 0.37.12 — pinned chat tab's restored Claude session id from the
   *  serialized layout. Forwarded to `AgentChatPane` so restore is
   *  deterministic without a daemon roundtrip race. */
  restoredSessionId?: string
  onClose?: () => void
}

/**
 * Thin dispatcher. Replaces the pre-0.36.0 4-sub-tab AgentPane.
 *
 * - `__wt:<id>` → WorktreeDetailPane (Task / Chat / Review for a worktree)
 * - `section === 'chat'` → AgentChatPane (persistent Claude chat)
 * - default ('inbox' or unset) → AgentInboxPane (work-queue kanban)
 *
 * CLAUDE.md and AGENT.md edit surfaces moved out — they live in
 * Workspace Settings now (the existing "Edit AGENT.md" / "Edit CLAUDE.md"
 * buttons there cover what the deleted sub-tabs did).
 */
export function AgentPane({ agentName, projectPath, section, restoredSessionId }: AgentPaneProps): React.JSX.Element {
  if (agentName.startsWith('__wt:')) {
    return <WorktreeDetailPane worktreeId={agentName.slice(5)} projectPath={projectPath} />
  }

  if (section === 'chat') {
    return <AgentChatPane agentName={agentName} projectPath={projectPath} restoredSessionId={restoredSessionId} />
  }

  // Default to inbox for legacy serialized rows that have no section field.
  return <AgentInboxPane agentName={agentName} projectPath={projectPath} />
}

// ── Worktree Detail Pane ────────────────────────────────────────────────
//
// Worktrees keep their own three-tab UI (Task / Chat / Review) — they
// were never pinned tabs and the workflow benefits from the bundled
// surface. The chat sub-tab uses the project-namespaced terminal id
// (`agent-chat:<project_id>:wt-<worktree_id>`) to avoid colliding with
// other worktrees' chat sessions.

const worktreeLastTab = new Map<string, 'task' | 'chat' | 'review'>()

// ── Review tab types (mirrors k2so_core::agents::reviews::ReviewItem) ─

interface WorktreeReviewDiffFile {
  path: string
  status: string
  additions: number
  deletions: number
}

interface WorktreeReviewWorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  itemType: string
  folder: string
}

interface WorktreeReviewItem {
  agentName: string
  branch: string
  worktreePath: string | null
  workItems: WorktreeReviewWorkItem[]
  diffSummary: WorktreeReviewDiffFile[]
}

function WorktreeDetailPane({ worktreeId, projectPath }: { worktreeId: string; projectPath: string }): React.JSX.Element {
  const [activeTab, setActiveTab] = useState<'task' | 'chat' | 'review'>(
    worktreeLastTab.get(worktreeId) ?? 'chat'
  )
  // Phase 2.1 wrap-up — Task tab reads `<worktree>/CLAUDE.md` via the
  // `read_worktree_file` Tauri command (path-canonicalized, traversal-
  // rejecting). Review tab fetches `k2so_agents_review_queue` and
  // filters client-side by this worktree's branch.
  const [taskContent, setTaskContent] = useState<string>('')
  const [taskError, setTaskError] = useState<string | null>(null)
  const [taskLoaded, setTaskLoaded] = useState(false)
  const [reviewItem, setReviewItem] = useState<WorktreeReviewItem | null>(null)
  const [reviewLoaded, setReviewLoaded] = useState(false)
  const [chatMounted, setChatMounted] = useState(activeTab === 'chat')

  const workspace = useProjectsStore(useCallback((s) => {
    for (const p of s.projects) {
      const ws = p.workspaces.find((w) => w.id === worktreeId)
      if (ws) return ws
    }
    return null
  }, [worktreeId]))
  const projectId = useProjectsStore(useCallback((s) => {
    for (const p of s.projects) {
      if (p.workspaces.some((w) => w.id === worktreeId)) return p.id
    }
    return null
  }, [worktreeId]))
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)

  const displayName = workspace?.name?.replace(/^agent\/[^/]+\//, '') || workspace?.branch || 'Worktree'
  const agentMatch = workspace?.name?.match(/^agent\/([^/]+)\//)
  const agentTemplate = agentMatch?.[1]
  const worktreePath = workspace?.worktreePath || projectPath
  const branch = workspace?.branch ?? null

  // Task tab — load <worktree>/CLAUDE.md whenever the worktree changes.
  // The fetch is gated on `worktreePath` being a non-empty absolute path
  // (the Tauri command rejects non-absolute paths anyway, but skipping
  // the call when we know it's the bare projectPath fallback avoids a
  // noisy "not_found" round-trip).
  useEffect(() => {
    let cancelled = false
    setTaskLoaded(false)
    setTaskContent('')
    setTaskError(null)
    if (!worktreePath || worktreePath === projectPath) {
      // No worktree path resolved yet — leave empty state.
      setTaskLoaded(true)
      return () => { cancelled = true }
    }
    invoke<string>('read_worktree_file', {
      worktreePath,
      relativePath: 'CLAUDE.md',
    })
      .then((content) => {
        if (cancelled) return
        setTaskContent(content)
        setTaskLoaded(true)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        const msg = typeof err === 'string' ? err : String(err)
        // `not_found` is the expected empty-state case; anything else
        // is a real error worth surfacing.
        if (msg === 'not_found' || msg.includes('not_found')) {
          setTaskError(null)
        } else {
          setTaskError(msg)
        }
        setTaskLoaded(true)
      })
    return () => { cancelled = true }
  }, [worktreePath, projectPath])

  // Review tab — fetch the workspace's review queue and filter to just
  // this worktree's row. The daemon's `/cli/reviews` route already
  // returns `branch` + `worktreePath` per item, so filtering happens
  // in the renderer with no daemon change. Empty result → empty state.
  useEffect(() => {
    let cancelled = false
    setReviewLoaded(false)
    setReviewItem(null)
    if (!projectPath || !branch) {
      setReviewLoaded(true)
      return () => { cancelled = true }
    }
    invoke<WorktreeReviewItem[]>('k2so_agents_review_queue', { projectPath })
      .then((items) => {
        if (cancelled) return
        // Prefer worktreePath match (most precise). Fall back to
        // branch match for review items whose worktree was already
        // removed but whose done/ items remain.
        const matchByPath = worktreePath
          ? items.find((it) => it.worktreePath === worktreePath)
          : undefined
        const matchByBranch = items.find((it) => it.branch === branch)
        setReviewItem(matchByPath ?? matchByBranch ?? null)
        setReviewLoaded(true)
      })
      .catch((err: unknown) => {
        // Reviews failing should not break the pane — log + show
        // empty state. Matches the badge-fetch pattern called out in
        // memory/feedback_test_discipline.md as an acceptable
        // defensive-catch exception.
        console.warn('[WorktreeDetailPane] review_queue fetch failed:', err)
        if (cancelled) return
        setReviewItem(null)
        setReviewLoaded(true)
      })
    return () => { cancelled = true }
  }, [projectPath, branch, worktreePath])

  useEffect(() => {
    if (activeTab === 'chat') setChatMounted(true)
  }, [activeTab])

  const reviewAvailable = reviewItem !== null
  const taskDisabled = !taskContent && taskLoaded && !taskError

  const tabs: Array<{ key: 'task' | 'chat' | 'review'; label: string; disabled: boolean }> = [
    { key: 'task', label: 'Task', disabled: taskDisabled },
    { key: 'chat', label: 'Chat', disabled: false },
    { key: 'review', label: 'Review', disabled: !reviewAvailable },
  ]

  return (
    <div className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3">
        <div className="flex gap-0.5 flex-shrink-0">
          {tabs.map(({ key, label, disabled }) => (
            <button
              key={key}
              onClick={() => {
                if (!disabled) {
                  setActiveTab(key)
                  worktreeLastTab.set(worktreeId, key)
                }
              }}
              disabled={disabled}
              className={`px-3 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                activeTab === key
                  ? 'bg-[var(--color-accent)] text-white'
                  : disabled
                    ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)]/40 cursor-not-allowed'
                    : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {displayName}
        </span>
        {agentTemplate && (
          <span className="text-[9px] font-medium text-[var(--color-text-muted)] bg-white/5 px-1.5 py-0.5 flex-shrink-0">
            {agentTemplate}
          </span>
        )}

        <div className="flex-1" />

        {projectId && (
          <button
            onClick={() => {
              const currentTabId = useTabsStore.getState().activeTabId
              if (currentTabId) {
                useTabsStore.getState().removeTab(currentTabId)
              }
              addNavWorktree(worktreeId)
              setTimeout(() => setActiveWorkspace(projectId, worktreeId), 50)
            }}
            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer flex-shrink-0 flex items-center gap-1"
            title="Open this worktree as a full workspace with file tree and changes"
          >
            <svg className="w-3 h-3" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
              <path d="M7 2H3a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V9" />
              <path d="M14 2l-7 7" />
              <path d="M10 2h4v4" />
            </svg>
            Open Full Workspace
          </button>
        )}
      </div>

      <div className="flex-1 overflow-hidden relative">
        {activeTab === 'task' && (
          <div className="h-full overflow-y-auto p-4">
            {!taskLoaded ? (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)]">Loading task brief…</p>
              </div>
            ) : taskError ? (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)]">
                  Failed to load CLAUDE.md: {taskError}
                </p>
              </div>
            ) : taskContent ? (
              <div className="prose prose-sm prose-invert max-w-none">
                <Markdown remarkPlugins={[remarkGfm]}>{taskContent}</Markdown>
              </div>
            ) : (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)] text-center max-w-md leading-relaxed">
                  This worktree has no <code>CLAUDE.md</code>. The Task brief is
                  rendered here when the harness or <code>k2so delegate</code>
                  {' '}writes one to the worktree root.
                </p>
              </div>
            )}
          </div>
        )}

        {chatMounted && projectId && (
          <div
            className="absolute inset-0"
            style={{ zIndex: activeTab === 'chat' ? 1 : 0, visibility: activeTab === 'chat' ? 'visible' : 'hidden' }}
          >
            <WorktreeChatTerminal
              worktreeId={worktreeId}
              projectId={projectId}
              cwd={worktreePath}
              projectPath={projectPath}
              autoFocus={activeTab === 'chat'}
            />
          </div>
        )}

        {activeTab === 'review' && (
          <div className="h-full overflow-y-auto p-4">
            {!reviewLoaded ? (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)]">Loading review queue…</p>
              </div>
            ) : reviewItem ? (
              <div className="space-y-4">
                <div className="space-y-2">
                  <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
                    Completed work
                  </div>
                  {reviewItem.workItems.length === 0 ? (
                    <p className="text-xs text-[var(--color-text-muted)]">
                      Worktree branch has diffs but no work items in <code>done/</code>.
                    </p>
                  ) : (
                    <ul className="space-y-1">
                      {reviewItem.workItems.map((wi) => (
                        <li
                          key={wi.filename}
                          className="text-xs text-[var(--color-text-primary)] flex items-start gap-2"
                        >
                          <span className="text-[var(--color-text-muted)] flex-shrink-0">·</span>
                          <span className="flex-1 truncate">{wi.title}</span>
                          <span className="text-[9px] uppercase tracking-wide text-[var(--color-text-muted)] flex-shrink-0">
                            {wi.priority}
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>

                <div className="space-y-2">
                  <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
                    Diff vs main ({reviewItem.diffSummary.length} files)
                  </div>
                  {reviewItem.diffSummary.length === 0 ? (
                    <p className="text-xs text-[var(--color-text-muted)]">
                      No file changes detected.
                    </p>
                  ) : (
                    <ul className="space-y-0.5 font-mono">
                      {reviewItem.diffSummary.slice(0, 50).map((f) => (
                        <li key={f.path} className="text-[11px] flex items-center gap-2">
                          <span className="w-3 flex-shrink-0 text-[var(--color-text-muted)]">{f.status}</span>
                          <span className="flex-1 truncate text-[var(--color-text-primary)]">{f.path}</span>
                          <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
                            +{f.additions} -{f.deletions}
                          </span>
                        </li>
                      ))}
                      {reviewItem.diffSummary.length > 50 && (
                        <li className="text-[10px] text-[var(--color-text-muted)] pl-5">
                          … and {reviewItem.diffSummary.length - 50} more files
                        </li>
                      )}
                    </ul>
                  )}
                </div>

                <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed border-t border-[var(--color-border)] pt-4">
                  If the work is not right, go to the Chat tab to address the issue with the agent before merging.
                </p>
              </div>
            ) : (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)]">
                  No pending reviews for this worktree.
                </p>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

// Internal: worktree chat uses a separate id (`wt-<worktreeId>` agent name)
// so it doesn't share state with the main agent chat tab. Project-namespaced
// per the same scheme.
function WorktreeChatTerminal({
  worktreeId,
  projectId,
  cwd,
  projectPath,
  autoFocus,
}: {
  worktreeId: string
  projectId: string
  cwd: string
  projectPath: string
  autoFocus: boolean
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const agentName = `wt-${worktreeId}`
  const terminalIdRef = useRef(agentChatId(projectId, agentName))
  const [launchConfig, setLaunchConfig] = useState<{ command: string; args: string[]; cwd: string } | null>(null)
  const [ready, setReady] = useState(false)

  useEffect(() => {
    let cancelled = false
    const resolve = async (): Promise<void> => {
      const myTerminalId = terminalIdRef.current
      try {
        const exists = await terminalExists(myTerminalId)
        if (!cancelled && exists) {
          setLaunchConfig(null)
          setReady(true)
          return
        }
      } catch { /* fall through */ }
      try {
        const result = await invoke<{
          command: string
          args: string[]
          cwd: string
        }>('k2so_agents_build_launch', { projectPath, agentName })
        if (!cancelled && result) {
          setLaunchConfig({ command: result.command, args: result.args, cwd: result.cwd })
          daemonCliGet('agents/lock', { project: projectPath, agent: agentName, terminal_id: myTerminalId, owner: 'user' }).catch(() => {})
          setReady(true)
          return
        }
      } catch (err) {
        console.warn('[WorktreeChat] build_launch failed, falling back:', err)
      }
      if (!cancelled) {
        setLaunchConfig({ command: 'claude', args: ['--dangerously-skip-permissions'], cwd })
        daemonCliGet('agents/lock', { project: projectPath, agent: agentName, terminal_id: myTerminalId, owner: 'user' }).catch(() => {})
        setReady(true)
      }
    }
    resolve()
    return () => { cancelled = true }
  }, [agentName, projectPath, cwd])

  useEffect(() => {
    if (autoFocus && ready) {
      requestAnimationFrame(() => {
        const el = containerRef.current?.querySelector('[tabindex]') as HTMLElement | null
        el?.focus()
      })
    }
  }, [autoFocus, ready])

  if (!ready) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading session…
      </div>
    )
  }

  return (
    <div ref={containerRef} className="h-full">
      <TerminalPane
        terminalId={terminalIdRef.current}
        cwd={launchConfig?.cwd ?? cwd}
        command={launchConfig?.command}
        args={launchConfig?.args}
      />
    </div>
  )
}

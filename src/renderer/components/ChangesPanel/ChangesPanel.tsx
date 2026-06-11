import { useState, useMemo, useCallback } from 'react'
// Plan B — git stage/unstage/commit are host-aware daemon data: route them
// through the `/cli/git/*` HTTP layer (local OR remote) instead of the
// localhost-pinned Tauri `git_*` invoke proxy. POST bodies are camelCase
// (`filePath`). The old Tauri git commands emitted NO cross-window sync, so
// the local `refetch()` after each mutation is the full contract.
import { daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { useSettingsStore } from '@/stores/settings'
import { useGitInfo, useGitChanges } from '@/hooks/useGit'

// ── Status helpers ───────────────────────────────────────────────────────────

const STATUS_CONFIG = {
  modified: { label: 'Modified', color: 'text-yellow-400', icon: 'M', bg: 'bg-yellow-400/10' },
  added: { label: 'Added', color: 'text-green-400', icon: 'A', bg: 'bg-green-400/10' },
  deleted: { label: 'Deleted', color: 'text-red-400', icon: 'D', bg: 'bg-red-400/10' },
  untracked: { label: 'Untracked', color: 'text-neutral-400', icon: 'U', bg: 'bg-neutral-400/10' }
} as const

type FileStatus = keyof typeof STATUS_CONFIG

interface ChangeFile {
  path: string
  status: string
  staged: boolean
}

// ── Component ────────────────────────────────────────────────────────────────

export default function ChangesPanel(): React.JSX.Element {
  const [commitMsg, setCommitMsg] = useState('')
  const [committing, setCommitting] = useState(false)

  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)

  const activeProject = projects.find((p) => p.id === activeProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)
  const workspacePath = activeWorkspace?.worktreePath ?? activeProject?.path

  const { data: gitInfo } = useGitInfo(workspacePath)
  const { data: changes, refetch } = useGitChanges(workspacePath)

  // Split files into staged and unstaged groups
  const { staged, unstaged } = useMemo(() => {
    const s: ChangeFile[] = []
    const u: ChangeFile[] = []
    for (const file of changes) {
      if (file.staged) s.push(file)
      else u.push(file)
    }
    return { staged: s, unstaged: u }
  }, [changes])

  // ── Actions ──────────────────────────────────────────────────────────────

  const handleStage = useCallback(async (filePath: string) => {
    if (!workspacePath) return
    await daemonCliPost('git/stage', { path: workspacePath, filePath }).catch(console.error)
    refetch()
  }, [workspacePath, refetch])

  const handleUnstage = useCallback(async (filePath: string) => {
    if (!workspacePath) return
    await daemonCliPost('git/unstage', { path: workspacePath, filePath }).catch(console.error)
    refetch()
  }, [workspacePath, refetch])

  const handleStageAll = useCallback(async () => {
    if (!workspacePath) return
    await daemonCliPost('git/stage-all', { path: workspacePath }).catch(console.error)
    refetch()
  }, [workspacePath, refetch])

  const handleUnstageAll = useCallback(async () => {
    if (!workspacePath) return
    for (const file of staged) {
      await daemonCliPost('git/unstage', { path: workspacePath, filePath: file.path }).catch(console.error)
    }
    refetch()
  }, [workspacePath, staged, refetch])

  const handleCommit = useCallback(async () => {
    if (!workspacePath || !commitMsg.trim() || staged.length === 0) return
    setCommitting(true)
    try {
      await daemonCliPost('git/commit', { path: workspacePath, message: commitMsg.trim() })
      setCommitMsg('')
      refetch()
    } catch (e) {
      console.error('[changes] Commit failed:', e)
    } finally {
      setCommitting(false)
    }
  }, [workspacePath, commitMsg, staged.length, refetch])

  // ── AI Commit ────────────────────────────────────────────────────────

  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const isWorktree = activeWorkspace?.type === 'worktree'
  const branchName = gitInfo?.currentBranch ?? 'current branch'

  const handleAiCommit = useCallback((includeMerge: boolean) => {
    if (!workspacePath) return

    const preset = presets.find((p) => p.id === defaultAgent)
    if (!preset) {
      console.error('[changes] No preset found for default agent:', defaultAgent)
      return
    }

    const { command, args } = parseCommand(preset.command)

    // Build concise changed-files summary (agent will run git diff itself)
    const MAX_FILES = 80
    const fileLines = changes.slice(0, MAX_FILES).map((f) => `${f.status}: ${f.path}`)
    if (changes.length > MAX_FILES) {
      fileLines.push(`...and ${changes.length - MAX_FILES} more files`)
    }
    const changedSummary = fileLines.join('\n')

    let prompt = `Review the following changes in this repository and create a well-structured commit with an appropriate commit message.\n\nChanged files:\n${changedSummary}`

    if (includeMerge) {
      prompt += `\n\nAfter committing, merge the branch "${branchName}" back into main and resolve any conflicts. Once merged, remove the worktree with "git worktree remove" and delete the branch with "git branch -d ${branchName}".`
    }

    const tabsStore = useTabsStore.getState()
    const activeGroup = tabsStore.activeGroupIndex
    tabsStore.addTabToGroup(activeGroup, workspacePath, {
      title: includeMerge ? 'AI Commit & Merge' : 'AI Commit',
      command,
      args: [...args, prompt]
    })
  }, [workspacePath, changes, presets, defaultAgent, branchName])

  const handleOpenDiff = useCallback((filePath: string) => {
    const activeTab = useTabsStore.getState().getActiveTab()
    if (activeTab) {
      useTabsStore.getState().openDiffInPane(activeTab.id, filePath)
    }
  }, [])

  // ── Render helpers ───────────────────────────────────────────────────────

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">No workspace selected</p>
      </div>
    )
  }

  if (!gitInfo?.isRepo) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">Not a git repository</p>
      </div>
    )
  }

  const totalCount = changes.length

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Branch info header */}
      <div className="px-3 py-2 border-b border-[var(--color-border)]">
        <div className="flex items-center gap-2">
          <svg
            className="w-3.5 h-3.5 text-[var(--color-text-muted)] flex-shrink-0"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z"
            />
          </svg>
          <span className="text-xs font-medium text-[var(--color-text-primary)] truncate">
            {gitInfo.currentBranch}
          </span>
          {(gitInfo.ahead > 0 || gitInfo.behind > 0) && (
            <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
              {gitInfo.ahead > 0 && <span className="text-green-400">{'\u2191'}{gitInfo.ahead}</span>}
              {gitInfo.ahead > 0 && gitInfo.behind > 0 && ' '}
              {gitInfo.behind > 0 && <span className="text-red-400">{'\u2193'}{gitInfo.behind}</span>}
            </span>
          )}
        </div>
      </div>

      {/* File lists */}
      <div className="flex-1 overflow-y-auto">
        {totalCount === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-xs text-[var(--color-text-muted)]">Working tree clean</p>
          </div>
        ) : (
          <>
            {/* Staged Changes */}
            {staged.length > 0 && (
              <FileSection
                title="Staged Changes"
                count={staged.length}
                files={staged}
                action="unstage"
                onToggle={handleUnstage}
                onBulkAction={handleUnstageAll}
                bulkLabel="Unstage All"
                onClickFile={handleOpenDiff}
              />
            )}

            {/* Unstaged Changes */}
            <FileSection
              title="Changes"
              count={unstaged.length}
              files={unstaged}
              action="stage"
              onToggle={handleStage}
              onBulkAction={handleStageAll}
              bulkLabel="Stage All"
              onClickFile={handleOpenDiff}
            />
          </>
        )}
      </div>

      {/* AI Commit buttons — shown when there are changes */}
      {totalCount > 0 && (
        <div className="border-t border-[var(--color-border)] p-2 flex flex-col gap-1">
          <button
            className="w-full px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-colors cursor-pointer"
            onClick={() => handleAiCommit(false)}
          >
            AI Commit
          </button>
          {isWorktree && (
            <button
              className="w-full px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-colors cursor-pointer"
              onClick={() => handleAiCommit(true)}
            >
              AI Commit & Merge
            </button>
          )}
        </div>
      )}

      {/* Commit input — shown when there are staged files */}
      {staged.length > 0 && (
        <div className="border-t border-[var(--color-border)] p-2">
          <textarea
            className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 resize-none outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
            placeholder="Commit message..."
            rows={3}
            value={commitMsg}
            onChange={(e) => setCommitMsg(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                e.preventDefault()
                handleCommit()
              }
            }}
          />
          <button
            className="w-full mt-1 px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed"
            disabled={!commitMsg.trim() || committing}
            onClick={handleCommit}
          >
            {committing ? 'Committing...' : `Commit (${staged.length} file${staged.length !== 1 ? 's' : ''})`}
          </button>
        </div>
      )}
    </div>
  )
}

// ── File Section ─────────────────────────────────────────────────────────────

interface FileSectionProps {
  title: string
  count: number
  files: ChangeFile[]
  action: 'stage' | 'unstage'
  onToggle: (filePath: string) => void
  onBulkAction: () => void
  bulkLabel: string
  onClickFile: (filePath: string) => void
}

function FileSection({
  title, count, files, action, onToggle, onBulkAction, bulkLabel, onClickFile
}: FileSectionProps): React.JSX.Element {
  return (
    <div className="mb-1">
      {/* Section header */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--color-border)]">
        <span className="text-xs font-medium text-[var(--color-text-secondary)]">
          {title} ({count})
        </span>
        {count > 0 && (
          <button
            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
            onClick={onBulkAction}
          >
            {bulkLabel}
          </button>
        )}
      </div>

      {/* File entries */}
      {files.map((file) => {
        const status = (file.status as FileStatus) in STATUS_CONFIG
          ? (file.status as FileStatus)
          : 'modified'
        const config = STATUS_CONFIG[status]

        return (
          <div
            key={file.path}
            className="flex items-center gap-1.5 px-2 py-0.5 hover:bg-white/[0.04] group cursor-pointer"
            onClick={() => onClickFile(file.path)}
          >
            {/* Stage/Unstage button */}
            <button
              className="w-4 h-4 flex items-center justify-center text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] opacity-0 group-hover:opacity-100 shrink-0"
              onClick={(e) => {
                e.stopPropagation()
                onToggle(file.path)
              }}
              title={action === 'stage' ? 'Stage file' : 'Unstage file'}
            >
              {action === 'stage' ? '+' : '-'}
            </button>

            {/* Status badge */}
            <span
              className={`w-4 h-4 flex items-center justify-center text-[10px] font-bold ${config.color} ${config.bg} flex-shrink-0`}
            >
              {config.icon}
            </span>

            {/* File path */}
            <span className="text-xs text-[var(--color-text-secondary)] truncate flex-1">
              {file.path}
            </span>
          </div>
        )
      })}
    </div>
  )
}

import { useState, useEffect, useCallback, useRef, useMemo } from 'react'
// Plan B — git branch list + worktree create are host-aware daemon data:
// route them through the `/cli/git/*` HTTP layer (local OR remote) instead
// of the localhost-pinned Tauri `git_*` invoke proxy. GET params snake_case
// (`path`); POST body camelCase (`projectPath`/`existingBranch`). The daemon
// writes the workspaces row + returns `{workspaceId,path,branch}`. The old
// Tauri git command emitted NO cross-window sync (optimistic local update
// is the contract).
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'

/** Sanitize a string into a valid git branch name */
function sanitizeBranchName(input: string): string {
  return input
    .trim()
    .replace(/\s+/g, '-')        // spaces → hyphens
    .replace(/\.{2,}/g, '-')     // consecutive dots → hyphen
    .replace(/[\x00-\x1f\x7f~^:?*[\]\\]/g, '') // control chars + git-invalid chars
    .replace(/\/\//g, '/')       // collapse double slashes
    .replace(/^[.\-/]+/, '')     // no leading dot, hyphen, or slash
    .replace(/[.\-/]+$/, '')     // no trailing dot, hyphen, or slash
    .replace(/\.lock$/i, '')     // can't end with .lock
}

interface WorktreeDialogProps {
  projectId: string
  projectPath: string
  open: boolean
  onClose: () => void
}

interface BranchList {
  current: string
  local: string[]
  remote: string[]
}

export default function WorktreeDialog({
  projectId,
  projectPath,
  open,
  onClose
}: WorktreeDialogProps): React.JSX.Element | null {
  const [name, setName] = useState('')
  const [mode, setMode] = useState<'new' | 'existing'>('new')
  const [branches, setBranches] = useState<string[]>([])
  const [selectedBranch, setSelectedBranch] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  const openAgentPane = useTabsStore((s) => s.openAgentPane)

  // Reset state and auto-focus when dialog opens
  useEffect(() => {
    if (!open) return
    setName('')
    setMode('new')
    setSelectedBranch('')
    setError(null)
    setCreating(false)
    // Focus after a tick so the input is mounted
    requestAnimationFrame(() => inputRef.current?.focus())

    // Fetch available branches
    daemonCliGet<BranchList>('git/branches', { path: projectPath })
      .then((result) => {
        setBranches(result.local.filter((b) => b !== result.current))
      })
      .catch(() => setBranches([]))
  }, [open, projectPath])

  // Close on Escape
  useEffect(() => {
    if (!open) return

    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [open, onClose])

  const sanitizedName = useMemo(() => sanitizeBranchName(name), [name])
  const namesDiffer = name.trim().length > 0 && sanitizedName !== name.trim()

  const handleCreate = useCallback(async () => {
    const branchName = mode === 'new' ? sanitizedName : selectedBranch
    if (!branchName) return

    setCreating(true)
    setError(null)

    try {
      const result = await daemonCliPost<{ workspaceId: string; path: string; branch: string }>(
        'git/create-worktree',
        {
          projectPath,
          branch: branchName,
          projectId,
          existingBranch: mode === 'existing'
        }
      )

      // Optimistic local update — never call fetchProjects() in render-adjacent code
      const state = useProjectsStore.getState()
      const updated = state.projects.map((p) => {
        if (p.id !== projectId) return p
        return {
          ...p,
          workspaces: [...p.workspaces, {
            id: result.workspaceId,
            projectId,
            name: result.branch,
            type: 'worktree' as const,
            branch: result.branch,
            worktreePath: result.path,
            tabOrder: p.workspaces.length,
            navVisible: 0,
            createdAt: Date.now(),
          }]
        }
      })
      useProjectsStore.setState({ projects: updated })
      // Open the worktree detail pane (Task/Chat/Review) without switching workspaces
      openAgentPane(`__wt:${result.workspaceId}`, result.path, result.branch)
      onClose()
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e instanceof Error ? e.message : 'Failed to create workspace')
      setError(msg)
    } finally {
      setCreating(false)
    }
  }, [sanitizedName, selectedBranch, mode, projectPath, projectId, openAgentPane, onClose])

  if (!open) return null

  const canCreate = mode === 'new'
    ? sanitizedName.length > 0 && !creating
    : selectedBranch.length > 0 && !creating

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" />

      {/* Dialog */}
      <div className="relative w-[350px] flex flex-col bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-2xl overflow-hidden">
        {/* Header */}
        <div className="px-4 py-3 border-b border-[var(--color-border)] flex items-center justify-between">
          <h2 className="text-sm font-semibold text-[var(--color-text-primary)]">
            New Worktree
          </h2>
          <button
            className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
            onClick={onClose}
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        {/* Mode toggle */}
        <div className="px-4 pt-3 flex gap-1">
          <button
            className={`flex-1 px-2 py-1.5 text-[11px] font-medium transition-colors ${
              mode === 'new'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)] border border-[var(--color-accent)]/30'
                : 'bg-white/[0.04] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-secondary)]'
            }`}
            onClick={() => setMode('new')}
          >
            New branch
          </button>
          <button
            className={`flex-1 px-2 py-1.5 text-[11px] font-medium transition-colors ${
              mode === 'existing'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)] border border-[var(--color-accent)]/30'
                : 'bg-white/[0.04] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-secondary)]'
            }`}
            onClick={() => setMode('existing')}
            disabled={branches.length === 0}
            title={branches.length === 0 ? 'No other local branches available' : undefined}
          >
            Existing branch
          </button>
        </div>

        {/* Content */}
        <div className="px-4 py-4">
          {mode === 'new' ? (
            <>
              <label className="text-xs text-[var(--color-text-muted)] block mb-1.5">
                Branch name
              </label>
              <input
                ref={inputRef}
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="feature/my-feature"
                className="w-full px-3 py-2 text-xs font-mono bg-white/[0.04] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)]/50 focus:ring-1 focus:ring-[var(--color-accent)]/25"
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && canCreate) handleCreate()
                }}
              />
              {namesDiffer && (
                <p className="text-[11px] text-[var(--color-text-secondary)] mt-1.5 font-mono">
                  <span className="text-[var(--color-text-muted)]">Branch: </span>
                  <span className="text-[var(--color-accent)]">{sanitizedName}</span>
                </p>
              )}
              <p className="text-[11px] text-[var(--color-text-muted)] mt-1.5">
                Creates a new branch and workspace from the current branch.
              </p>
            </>
          ) : (
            <>
              <label className="text-xs text-[var(--color-text-muted)] block mb-1.5">
                Select branch
              </label>
              <select
                value={selectedBranch}
                onChange={(e) => setSelectedBranch(e.target.value)}
                className="w-full px-3 py-2 text-xs font-mono bg-white/[0.04] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]/50 focus:ring-1 focus:ring-[var(--color-accent)]/25"
              >
                <option value="" disabled>Choose a branch...</option>
                {branches.map((b) => (
                  <option key={b} value={b}>{b}</option>
                ))}
              </select>
              <p className="text-[11px] text-[var(--color-text-muted)] mt-2">
                Opens an existing branch in its own workspace.
              </p>
            </>
          )}
        </div>

        {/* Error */}
        {error && (
          <div className="px-4 py-2 text-xs text-red-400 bg-red-400/5 border-t border-red-400/10">
            {error}
          </div>
        )}

        {/* Footer */}
        <div className="px-4 py-3 border-t border-[var(--color-border)] flex justify-end gap-2">
          <button
            className="px-3 py-1.5 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
            onClick={onClose}
            disabled={creating}
          >
            Cancel
          </button>
          <button
            className={`px-4 py-1.5 text-xs font-medium transition-colors ${
              canCreate
                ? 'bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90'
                : 'bg-white/[0.06] text-[var(--color-text-muted)] cursor-not-allowed'
            }`}
            onClick={handleCreate}
            disabled={!canCreate}
          >
            {creating ? (
              <span className="flex items-center gap-2">
                <div className="w-3 h-3 border-2 border-white/40 border-t-white rounded-full animate-spin" />
                Creating...
              </span>
            ) : (
              'Create'
            )}
          </button>
        </div>
      </div>
    </div>
  )
}

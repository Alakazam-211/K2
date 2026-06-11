import { useState, useEffect, useCallback } from 'react'
// Plan B — git branch/diff/merge/worktree ops are host-aware daemon data:
// route them through the `/cli/git/*` HTTP layer (local OR remote) instead
// of the localhost-pinned Tauri `git_*` invoke proxy. GET params are
// snake_case (`base_branch`/`head_branch`); POST bodies are camelCase. The
// old Tauri git commands emitted NO cross-window sync, so the explicit
// `fetchProjects()` after cleanup is the full contract.
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'

// ── Types ────────────────────────────────────────────────────────────────

interface FileDiffSummary {
  path: string
  status: string
  additions: number
  deletions: number
  oldPath: string | null
}

interface MergeResult {
  success: boolean
  conflicts: string[]
  mergedFiles: number
}

type MergeStep = 'preview' | 'merging' | 'success' | 'conflicts'

// ── Store ────────────────────────────────────────────────────────────────

import { create } from 'zustand'

interface MergeDialogStore {
  open: boolean
  branch: string
  projectPath: string
  projectId: string
  workspaceId: string | null
  show: (branch: string, projectPath: string, projectId: string, workspaceId: string | null) => void
  close: () => void
}

export const useMergeDialogStore = create<MergeDialogStore>((set) => ({
  open: false,
  branch: '',
  projectPath: '',
  projectId: '',
  workspaceId: null,
  show: (branch, projectPath, projectId, workspaceId) =>
    set({ open: true, branch, projectPath, projectId, workspaceId }),
  close: () => set({ open: false }),
}))

// ── Component ────────────────────────────────────────────────────────────

export default function MergeDialog(): React.JSX.Element | null {
  const { open, branch, projectPath, projectId, workspaceId, close } = useMergeDialogStore()
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  const [step, setStep] = useState<MergeStep>('preview')
  const [diffs, setDiffs] = useState<FileDiffSummary[]>([])
  const [conflicts, setConflicts] = useState<string[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Load preview when dialog opens
  useEffect(() => {
    if (!open || !projectPath || !branch) return
    setStep('preview')
    setError(null)
    setLoading(true)

    // Get the current branch to diff against
    daemonCliGet<{ currentBranch: string }>('git/info', { path: projectPath })
      .then((info) => {
        return daemonCliGet<FileDiffSummary[]>('git/diff-between', {
          path: projectPath,
          base_branch: info.currentBranch,
          head_branch: branch,
        })
      })
      .then((result) => {
        setDiffs(result)
        setLoading(false)
      })
      .catch((e) => {
        setError(String(e))
        setLoading(false)
      })
  }, [open, projectPath, branch])

  const cleanupWorktree = useCallback(async () => {
    try {
      const project = useProjectsStore.getState().projects.find((p) => p.id === projectId)
      const workspace = project?.workspaces.find((w) => w.id === workspaceId)
      const worktreePath = workspace?.worktreePath

      if (worktreePath) {
        await daemonCliPost('git/remove-worktree', {
          projectPath,
          worktreePath,
          workspaceId,
          force: false,
        })
      }

      // Delete the branch
      await daemonCliPost('git/delete-branch', { path: projectPath, branch }).catch((e) => console.warn('[merge-dialog]', e))

      await fetchProjects()
    } catch (e) {
      console.error('[merge] Cleanup failed:', e)
    }
  }, [projectPath, projectId, workspaceId, branch, fetchProjects])

  const handleMerge = useCallback(async () => {
    setStep('merging')
    setError(null)

    try {
      const result = await daemonCliPost<MergeResult>('git/merge-branch', {
        path: projectPath,
        branch,
      })

      if (result.success) {
        // Auto-cleanup: move worktree to Trash and delete branch
        if (workspaceId) {
          await cleanupWorktree()
        }
        setStep('success')
      } else {
        setConflicts(result.conflicts)
        setStep('conflicts')
      }
    } catch (e) {
      setError(String(e))
      setStep('preview')
    }
  }, [projectPath, branch, workspaceId, cleanupWorktree])

  const handleAbortMerge = useCallback(async () => {
    await daemonCliPost('git/abort-merge', { path: projectPath }).catch(console.error)
    close()
  }, [projectPath, close])

  const handleDone = useCallback(() => {
    fetchProjects()
    close()
  }, [fetchProjects, close])

  if (!open) return null

  const totalAdditions = diffs.reduce((sum, d) => sum + d.additions, 0)
  const totalDeletions = diffs.reduce((sum, d) => sum + d.deletions, 0)

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={close}>
      <div
        className="bg-[var(--color-bg-surface)] border border-[var(--color-border)] w-[520px] max-h-[80vh] flex flex-col shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-4 py-3 border-b border-[var(--color-border)]">
          <h2 className="text-sm font-semibold text-[var(--color-text-primary)]">
            Merge &ldquo;{branch}&rdquo; into current branch
          </h2>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-4">
          {step === 'preview' && (
            <>
              {loading ? (
                <p className="text-xs text-[var(--color-text-muted)]">Loading diff preview...</p>
              ) : error ? (
                <p className="text-xs text-red-400">{error}</p>
              ) : diffs.length === 0 ? (
                <p className="text-xs text-[var(--color-text-muted)]">No changes to merge — branches are identical.</p>
              ) : (
                <>
                  {/* Summary */}
                  <div className="flex items-center gap-3 mb-3">
                    <span className="text-xs text-[var(--color-text-secondary)]">
                      {diffs.length} file{diffs.length !== 1 ? 's' : ''} changed
                    </span>
                    {totalAdditions > 0 && (
                      <span className="text-xs text-green-400 font-mono">+{totalAdditions}</span>
                    )}
                    {totalDeletions > 0 && (
                      <span className="text-xs text-red-400 font-mono">-{totalDeletions}</span>
                    )}
                  </div>

                  {/* File list */}
                  <div className="border border-[var(--color-border)]">
                    {diffs.map((file) => (
                      <div
                        key={file.path}
                        className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0"
                      >
                        <span className={`text-[10px] font-bold w-4 text-center ${
                          file.status === 'added' ? 'text-green-400' :
                          file.status === 'deleted' ? 'text-red-400' :
                          file.status === 'renamed' ? 'text-blue-400' :
                          'text-yellow-400'
                        }`}>
                          {file.status[0].toUpperCase()}
                        </span>
                        <span className="text-xs text-[var(--color-text-secondary)] truncate flex-1">
                          {file.path}
                        </span>
                        <span className="text-[10px] font-mono text-green-400">+{file.additions}</span>
                        <span className="text-[10px] font-mono text-red-400">-{file.deletions}</span>
                      </div>
                    ))}
                  </div>
                </>
              )}
            </>
          )}

          {step === 'merging' && (
            <div className="flex items-center gap-2">
              <div className="w-3 h-3 border-2 border-[var(--color-accent)] border-t-transparent rounded-full animate-spin" />
              <span className="text-xs text-[var(--color-text-secondary)]">Merging...</span>
            </div>
          )}

          {step === 'success' && (
            <div>
              <p className="text-xs text-green-400 mb-3">
                Merge successful! Branch &ldquo;{branch}&rdquo; has been merged.
              </p>
              {workspaceId && (
                <p className="text-xs text-[var(--color-text-muted)]">
                  Worktree and branch have been cleaned up. Files were moved to Trash for recovery if needed.
                </p>
              )}
            </div>
          )}

          {step === 'conflicts' && (
            <div>
              <p className="text-xs text-yellow-400 mb-3">
                Merge has conflicts in {conflicts.length} file{conflicts.length !== 1 ? 's' : ''}. Resolve them in the editor, then stage and commit.
              </p>
              <div className="border border-[var(--color-border)]">
                {conflicts.map((path) => (
                  <div key={path} className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0">
                    <span className="text-[10px] font-bold text-red-400">!</span>
                    <span className="text-xs text-[var(--color-text-secondary)] truncate">{path}</span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="px-4 py-3 border-t border-[var(--color-border)] flex justify-end gap-2">
          {step === 'preview' && (
            <>
              <button
                className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
                onClick={close}
              >
                Cancel
              </button>
              <button
                className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 disabled:opacity-40"
                disabled={loading || diffs.length === 0}
                onClick={handleMerge}
              >
                Merge
              </button>
            </>
          )}

          {step === 'success' && (
            <button
              className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90"
              onClick={handleDone}
            >
              Done
            </button>
          )}

          {step === 'conflicts' && (
            <>
              <button
                className="px-3 py-1.5 text-xs text-red-400 hover:text-red-300"
                onClick={handleAbortMerge}
              >
                Abort Merge
              </button>
              <button
                className="px-3 py-1.5 text-xs font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
                onClick={close}
              >
                Resolve Later
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  )
}

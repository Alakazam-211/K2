import { useState, useEffect, useCallback } from 'react'
import { emit } from '@tauri-apps/api/event'
// Plan B — projects_update / workspaces_delete AND the git cluster
// (git_changes read + git_remove_worktree mutation) are host-aware daemon
// data: route through the `/cli/*` HTTP layer (local OR remote). GET params
// snake_case (`path`); POST bodies camelCase. The git commands emitted NO
// cross-window sync, so the explicit `sync:projects` emits here are for the
// projects/workspaces mutations, not git.
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore, type ProjectWithWorkspaces } from '../../stores/projects'
import { useActiveAgentsStore } from '@/stores/active-agents'
import AgentCloseDialog from '@/components/AgentCloseDialog/AgentCloseDialog'

interface DisableWorktreesDialogProps {
  project: ProjectWithWorkspaces
  open: boolean
  onClose: () => void
}

type Action = 'cancel' | 'conceal' | 'close' | 'recycle'

export default function DisableWorktreesDialog({
  project,
  open,
  onClose
}: DisableWorktreesDialogProps): React.JSX.Element | null {
  const [isPending, setIsPending] = useState(false)
  const [action, setAction] = useState<Action | null>(null)
  const [hasUnmerged, setHasUnmerged] = useState(false)
  const [showRecycleConfirm, setShowRecycleConfirm] = useState(false)
  const [showAgentDialog, setShowAgentDialog] = useState(false)
  const [pendingAgentAction, setPendingAgentAction] = useState<Action | null>(null)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  // Get worktree-type workspaces (not the main branch workspace)
  const worktrees = project.workspaces.filter((ws) => ws.type === 'worktree')
  const worktreeCount = worktrees.length

  // Check for unmerged changes when dialog opens
  useEffect(() => {
    if (!open || worktreeCount === 0) return

    let cancelled = false
    const checkChanges = async (): Promise<void> => {
      for (const ws of worktrees) {
        if (!ws.worktreePath) continue
        try {
          const changes = await daemonCliGet<any[]>('git/changes', { path: ws.worktreePath })
          if (!cancelled && changes.length > 0) {
            setHasUnmerged(true)
            return
          }
        } catch {
          // ignore
        }
      }
    }
    checkChanges()
    return () => { cancelled = true }
  }, [open, worktreeCount])

  // Reset state when dialog opens
  useEffect(() => {
    if (open) {
      setAction(null)
      setIsPending(false)
      setShowRecycleConfirm(false)
      setHasUnmerged(false)
      setShowAgentDialog(false)
      setPendingAgentAction(null)
    }
  }, [open])

  const handleAction = useCallback(async (selectedAction: Action, bypassAgentCheck = false) => {
    if (selectedAction === 'cancel') {
      onClose()
      return
    }

    // Check for active agents before any destructive action
    if (!bypassAgentCheck) {
      const agents = useActiveAgentsStore.getState().getActiveAgentsList()
      if (agents.length > 0) {
        setPendingAgentAction(selectedAction)
        setShowAgentDialog(true)
        return
      }
    }

    if (selectedAction === 'recycle' && hasUnmerged && !showRecycleConfirm) {
      setShowRecycleConfirm(true)
      return
    }

    setAction(selectedAction)
    setIsPending(true)

    try {
      if (selectedAction === 'conceal') {
        // Just disable worktreeMode — keep all workspace records
        await daemonCliPost('projects/update', { id: project.id, worktreeMode: 0 })
        void emit('sync:projects').catch(() => {})
      } else if (selectedAction === 'close') {
        // Delete worktree workspace records, keep files on disk
        for (const ws of worktrees) {
          await daemonCliPost('workspaces/delete', { id: ws.id })
        }
        await daemonCliPost('projects/update', { id: project.id, worktreeMode: 0 })
        void emit('sync:projects').catch(() => {})
      } else if (selectedAction === 'recycle') {
        // Trash worktree folders + remove workspace records
        for (const ws of worktrees) {
          if (ws.worktreePath) {
            try {
              await daemonCliPost('git/remove-worktree', {
                projectPath: project.path,
                worktreePath: ws.worktreePath,
                workspaceId: ws.id
              })
            } catch {
              // If git remove fails, just delete the record
              await daemonCliPost('workspaces/delete', { id: ws.id })
            }
          } else {
            await daemonCliPost('workspaces/delete', { id: ws.id })
          }
        }
        await daemonCliPost('projects/update', { id: project.id, worktreeMode: 0 })
        void emit('sync:projects').catch(() => {})
      }

      await fetchProjects()
      onClose()
    } catch (err) {
      console.error('[disable-worktrees] Failed:', err)
      setIsPending(false)
    }
  }, [project.id, worktrees, hasUnmerged, showRecycleConfirm, fetchProjects, onClose])

  // Escape to close
  useEffect(() => {
    if (!open) return
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !isPending) onClose()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [open, isPending, onClose])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={isPending ? undefined : onClose}
    >
      <div
        className="w-[520px] border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-2">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            Disable Worktrees
          </h2>
        </div>

        {/* Body */}
        <div className="px-5 pb-4">
          <p className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
            <span className="text-[var(--color-text-primary)] font-medium">{project.name}</span>{' '}
            has {worktreeCount} active worktree{worktreeCount !== 1 ? 's' : ''}.
            How would you like to handle them?
          </p>

          {/* Worktree list */}
          <div className="mt-3 border border-[var(--color-border)]">
            {worktrees.map((ws, i) => (
              <div
                key={ws.id}
                className={`flex items-center gap-2 px-3 py-1.5 text-xs ${
                  i < worktrees.length - 1 ? 'border-b border-[var(--color-border)]' : ''
                }`}
              >
                <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-muted)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z" />
                </svg>
                <span className="text-[var(--color-text-secondary)] truncate flex-1">{ws.name}</span>
                {ws.branch && (
                  <span className="text-[10px] text-[var(--color-text-muted)] font-mono truncate max-w-[120px]">{ws.branch}</span>
                )}
              </div>
            ))}
          </div>
        </div>

        {/* Recycle warning for unmerged changes */}
        {showRecycleConfirm && (
          <div className="px-5 pb-4">
            <div className="border border-yellow-500/30 bg-yellow-500/10 px-3 py-2">
              <p className="text-[11px] text-yellow-400 font-medium mb-1">Warning: Unmerged Changes Detected</p>
              <p className="text-[11px] text-yellow-400/80">
                One or more worktrees have uncommitted or unmerged changes.
                Recycling will move these folders to Trash permanently.
              </p>
              <div className="flex items-center gap-2 mt-2">
                <button
                  onClick={() => handleAction('recycle')}
                  disabled={isPending}
                  className="px-3 py-1 text-xs text-red-400 border border-red-500/30 bg-red-500/10 hover:bg-red-500/20 transition-colors disabled:opacity-40 no-drag cursor-pointer"
                >
                  {isPending && action === 'recycle' ? 'Recycling...' : 'Confirm Recycle'}
                </button>
                <button
                  onClick={() => setShowRecycleConfirm(false)}
                  disabled={isPending}
                  className="px-3 py-1 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors disabled:opacity-40 no-drag cursor-pointer"
                >
                  Back
                </button>
              </div>
            </div>
          </div>
        )}

        {/* Actions — 4 buttons + cancel */}
        {!showRecycleConfirm && (
          <div className="px-5 pb-5 flex flex-col gap-2">
            {/* Conceal */}
            <button
              onClick={() => handleAction('conceal')}
              disabled={isPending}
              className="w-full px-3 py-2 text-xs text-left flex items-start gap-3 bg-white/[0.04] hover:bg-white/[0.08] border border-[var(--color-border)] transition-colors disabled:opacity-40 no-drag cursor-pointer"
            >
              <span className="text-[var(--color-text-primary)] font-medium flex-shrink-0 w-14">Conceal</span>
              <span className="text-[var(--color-text-muted)]">
                Hide worktrees from the sidebar but keep them in the database. Re-enable worktrees later to see them again.
              </span>
            </button>

            {/* Close */}
            <button
              onClick={() => handleAction('close')}
              disabled={isPending}
              className="w-full px-3 py-2 text-xs text-left flex items-start gap-3 bg-white/[0.04] hover:bg-white/[0.08] border border-[var(--color-border)] transition-colors disabled:opacity-40 no-drag cursor-pointer"
            >
              <span className="text-[var(--color-text-primary)] font-medium flex-shrink-0 w-14">Close</span>
              <span className="text-[var(--color-text-muted)]">
                Remove worktrees from the sidebar but leave the files on disk. Reopen them later from workspace settings.
              </span>
            </button>

            {/* Recycle */}
            <button
              onClick={() => handleAction('recycle')}
              disabled={isPending}
              className="w-full px-3 py-2 text-xs text-left flex items-start gap-3 bg-red-500/5 hover:bg-red-500/10 border border-red-500/20 transition-colors disabled:opacity-40 no-drag cursor-pointer"
            >
              <span className="text-red-400 font-medium flex-shrink-0 w-14">Recycle</span>
              <span className="text-[var(--color-text-muted)]">
                Move worktree folders to Trash and remove from the sidebar. This cannot be undone.
              </span>
            </button>

            {/* Cancel */}
            <button
              onClick={onClose}
              disabled={isPending}
              className="w-full px-3 py-1.5 text-xs text-center text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors disabled:opacity-40 no-drag cursor-pointer"
            >
              Cancel
            </button>
          </div>
        )}

        {/* Loading indicator */}
        {isPending && !showRecycleConfirm && (
          <div className="px-5 pb-4">
            <p className="text-xs text-[var(--color-text-muted)]">
              {action === 'conceal' ? 'Concealing' : action === 'close' ? 'Closing' : 'Recycling'} worktrees...
            </p>
          </div>
        )}
      </div>

      {showAgentDialog && pendingAgentAction && (
        <AgentCloseDialog
          agents={useActiveAgentsStore.getState().getActiveAgentsList()}
          mode="tab"
          onConfirm={() => {
            setShowAgentDialog(false)
            const act = pendingAgentAction
            setPendingAgentAction(null)
            handleAction(act, true)
          }}
          onCancel={() => {
            setShowAgentDialog(false)
            setPendingAgentAction(null)
          }}
        />
      )}
    </div>
  )
}

import { useState, useCallback, useEffect, useRef } from 'react'
import { useGitInitDialogStore } from '../../stores/git-init-dialog'
import { useProjectsStore } from '../../stores/projects'
import { useTabsStore, ensurePinnedAgentTabForMode } from '../../stores/tabs'
import { useFocusGroupsStore } from '../../stores/focus-groups'
import { emit } from '@tauri-apps/api/event'
// Plan B — project add / focus-group assign are host-aware daemon data:
// route through the `/cli/*` HTTP layer and re-emit the same cross-window
// `sync:projects` / `sync:focus-groups` events the old Tauri shims fired.
import { daemonCliPost } from '@/lib/daemon-cli'

export default function GitInitDialog(): React.JSX.Element | null {
  const isOpen = useGitInitDialogStore((s) => s.isOpen)
  const isPending = useGitInitDialogStore((s) => s.isPending)
  const path = useGitInitDialogStore((s) => s.path)
  const name = useGitInitDialogStore((s) => s.name)
  const error = useGitInitDialogStore((s) => s.error)
  const close = useGitInitDialogStore((s) => s.close)
  const setIsPending = useGitInitDialogStore((s) => s.setIsPending)
  const setError = useGitInitDialogStore((s) => s.setError)

  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  const [branchName, setBranchName] = useState('main')
  // Capture project IDs before the dialog action so we can find the new one
  const prevIdsRef = useRef<Set<string>>(new Set())

  // Reset branch name and capture current project IDs when dialog opens
  useEffect(() => {
    if (isOpen) {
      setBranchName('main')
      prevIdsRef.current = new Set(useProjectsStore.getState().projects.map((p) => p.id))
    }
  }, [isOpen])

  const selectNewProject = useCallback(async () => {
    // Stash current workspace before switching
    const preState = useProjectsStore.getState()
    const tabsStore = useTabsStore.getState()
    if (preState.activeProjectId && preState.activeWorkspaceId) {
      tabsStore.stashWorkspace(`${preState.activeProjectId}:${preState.activeWorkspaceId}`)
    } else if (useTabsStore.getState().tabs.length > 0) {
      tabsStore.clearAllTabs()
    }

    await fetchProjects()

    const state = useProjectsStore.getState()
    // Find the new project by comparing before/after IDs
    const newProject = state.projects.find((p) => !prevIdsRef.current.has(p.id))
      ?? state.projects[state.projects.length - 1]

    if (newProject) {
      // Assign to active focus group
      const focusState = useFocusGroupsStore.getState()
      const targetGroupId = focusState.focusGroupsEnabled ? focusState.activeFocusGroupId : null
      if (targetGroupId) {
        try {
          await daemonCliPost('focus-groups/assign', { projectId: newProject.id, focusGroupId: targetGroupId })
          // assign emitted BOTH sync:focus-groups and sync:projects.
          void emit('sync:focus-groups').catch(() => {})
          void emit('sync:projects').catch(() => {})
          useProjectsStore.setState({
            projects: useProjectsStore.getState().projects.map((p) =>
              p.id === newProject.id ? { ...p, focusGroupId: targetGroupId } : p
            )
          })
        } catch { /* non-fatal */ }
      }

      const newWorkspaceId = newProject.workspaces[0]?.id ?? null
      useProjectsStore.setState({
        activeProjectId: newProject.id,
        activeWorkspaceId: newWorkspaceId
      })

      if (newWorkspaceId) {
        const cwd = newProject.workspaces[0]?.worktreePath ?? newProject.path ?? '~'
        const newKey = `${newProject.id}:${newWorkspaceId}`
        // #681 (Bug A) — await restoreWorkspace, then ensure the pinned
        // Chat + Inbox tabs (this git-init-open path previously created no
        // pinned tabs at all, so a freshly git-init'd workspace opened
        // with neither surface visible). Mirrors addProject.
        await useTabsStore.getState().restoreWorkspace(newKey, cwd)
        ensurePinnedAgentTabForMode(newProject.agentMode, newProject.path)
      } else {
        useTabsStore.getState().clearAllTabs()
      }
    }
  }, [fetchProjects])

  // Initialize git and open
  const handleInitGit = useCallback(async () => {
    if (!path) return
    setIsPending(true)
    try {
      await daemonCliPost('projects/init-git-and-open', { path, branch: branchName })
      void emit('sync:projects').catch(() => {})
      close()
      await selectNewProject()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [path, branchName, close, selectNewProject, setIsPending, setError])

  // Open without git
  const handleOpenWithoutGit = useCallback(async () => {
    if (!path) return
    setIsPending(true)
    try {
      await daemonCliPost('projects/add-without-git', { path })
      void emit('sync:projects').catch(() => {})
      close()
      await selectNewProject()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [path, close, selectNewProject, setIsPending, setError])

  // Close on Escape
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !isPending) close()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, isPending, close])

  if (!isOpen || !path || !name) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={isPending ? undefined : close}
    >
      <div
        className="w-[440px] border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-2">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            New Workspace
          </h2>
        </div>

        {/* Body */}
        <div className="px-5 pb-4">
          <p className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
            <span className="text-[var(--color-text-primary)] font-medium">{name}</span>{' '}
            is not a git repository. How would you like to open it?
          </p>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1.5 break-all">
            {path}
          </p>
        </div>

        {/* Branch name input */}
        <div className="px-5 pb-4">
          <label className="text-[10px] text-[var(--color-text-muted)] block mb-1">
            Initial branch name (for git init)
          </label>
          <input
            type="text"
            value={branchName}
            onChange={(e) => setBranchName(e.target.value)}
            placeholder="main"
            className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]"
            disabled={isPending}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !isPending) handleInitGit()
            }}
          />
        </div>

        {/* Error */}
        {error && (
          <div className="px-5 pb-4">
            <div className="border border-red-500/30 bg-red-500/10 px-3 py-2">
              <p className="text-[11px] text-red-400 whitespace-pre-wrap">{error}</p>
            </div>
          </div>
        )}

        {/* Actions — three options */}
        <div className="px-5 pb-5 flex flex-col gap-2">
          {/* Primary: Initialize Git */}
          <button
            className="w-full px-3 py-2 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] border border-transparent transition-colors disabled:opacity-40 text-left flex items-center gap-2"
            onClick={handleInitGit}
            disabled={isPending || !branchName.trim()}
          >
            <svg className="w-3.5 h-3.5 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z" />
            </svg>
            {isPending ? 'Initializing...' : `Initialize Git (branch: ${branchName.trim() || 'main'})`}
          </button>

          {/* Secondary: Open without git */}
          <button
            className="w-full px-3 py-2 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-white/[0.04] hover:bg-white/[0.08] border border-[var(--color-border)] transition-colors disabled:opacity-40 text-left flex items-center gap-2"
            onClick={handleOpenWithoutGit}
            disabled={isPending}
          >
            <svg className="w-3.5 h-3.5 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M5 19a2 2 0 01-2-2V7a2 2 0 012-2h4l2 2h4a2 2 0 012 2v1M5 19h14a2 2 0 002-2v-5a2 2 0 00-2-2H9a2 2 0 00-2 2v5a2 2 0 01-2 2z" />
            </svg>
            Open Without Git
          </button>

          {/* Cancel */}
          <button
            className="w-full px-3 py-1.5 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors disabled:opacity-40 text-center"
            onClick={close}
            disabled={isPending}
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  )
}

import { useState, useCallback, useEffect } from 'react'
import { useGitInitDialogStore } from '../../stores/git-init-dialog'
import { revealAddedProject } from '../../stores/projects'
import { runWorkspaceIngestBestEffort } from '@/lib/add-workspace'
// Plan B — project add / focus-group assign are host-aware daemon data:
// route through the `/cli/*` HTTP layer; `revealAddedProject` re-emits the
// cross-window `sync:projects` / `sync:focus-groups` events.
import { daemonCliPost } from '@/lib/daemon-cli'
import { Callout, DialogScrim } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { GitToolsBanner } from '@/components/GitTools/GitToolsNotice'
import { ADD_WORKSPACE_Z } from '@/lib/overlay-layers'

export default function GitInitDialog(): React.JSX.Element | null {
  const isOpen = useGitInitDialogStore((s) => s.isOpen)
  const isPending = useGitInitDialogStore((s) => s.isPending)
  const path = useGitInitDialogStore((s) => s.path)
  const name = useGitInitDialogStore((s) => s.name)
  const error = useGitInitDialogStore((s) => s.error)
  const seedWiki = useGitInitDialogStore((s) => s.seedWiki)
  const seedAgentsMd = useGitInitDialogStore((s) => s.seedAgentsMd)
  const fanout = useGitInitDialogStore((s) => s.fanout)
  const close = useGitInitDialogStore((s) => s.close)
  const setIsPending = useGitInitDialogStore((s) => s.setIsPending)
  const setError = useGitInitDialogStore((s) => s.setError)

  const [branchName, setBranchName] = useState('main')

  // Reset branch name when dialog opens
  useEffect(() => {
    if (isOpen) setBranchName('main')
  }, [isOpen])

  // 0.46.10: show and select the workspace the daemon just added (its own
  // answer, inserted optimistically if the list read fails), then start the
  // skill write. The ingest is best-effort and not awaited.
  const showAdded = useCallback(async (added: unknown) => {
    const project = await revealAddedProject(added)
    if (project) void runWorkspaceIngestBestEffort(project.path)
  }, [])

  // Initialize git and open
  const handleInitGit = useCallback(async () => {
    if (!path) return
    setIsPending(true)
    try {
      const added = await daemonCliPost<unknown>(primaryScope(), 'projects/init-git-and-open', {
        path,
        // Trimmed here; the daemon trims and checks the name too (0.46.4).
        branch: branchName.trim(),
        seedWiki,
        seedAgentsMd,
        fanout,
      })
      close()
      await showAdded(added)
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [path, branchName, seedWiki, seedAgentsMd, fanout, close, showAdded, setIsPending, setError])

  // Open without git
  const handleOpenWithoutGit = useCallback(async () => {
    if (!path) return
    setIsPending(true)
    try {
      const added = await daemonCliPost<unknown>(primaryScope(), 'projects/add-without-git', { path, seedWiki, seedAgentsMd, fanout })
      close()
      await showAdded(added)
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [path, seedWiki, seedAgentsMd, fanout, close, showAdded, setIsPending, setError])

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
    <DialogScrim
      zIndex={ADD_WORKSPACE_Z}
      className="flex items-center justify-center no-drag"
      style={{ backdropFilter: 'blur(4px)' }}
      onClick={isPending ? undefined : close}
    >
      <div
        data-material="tile" data-floating="" className="w-[440px] border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
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

        {/* 0.46.4: no git on the server yet (a Mac without Apple's command
            line tools). Init still adds the workspace; the daemon does the
            git steps once git runs. */}
        <div className="px-5 pb-4 empty:hidden">
          <GitToolsBanner />
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
            <Callout tone="error">
              <p className="whitespace-pre-wrap">{error}</p>
            </Callout>
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
    </DialogScrim>
  )
}

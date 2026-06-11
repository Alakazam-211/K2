import React, { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import {
  useRemoveWorkspaceDialogStore,
  type RemoveWorkspaceMode,
  type RemoveWorkspaceResult,
} from '../../stores/remove-workspace-dialog'
import { useProjectsStore } from '../../stores/projects'

interface ModeOption {
  id: RemoveWorkspaceMode
  title: string
  description: string
}

const MODE_OPTIONS: ModeOption[] = [
  {
    id: 'deregister_only',
    title: 'Deregister only (default)',
    description:
      'Remove from K2\'s project list. Leave every file and symlink exactly where it is. Re-adding later picks up right where you left off.',
  },
  {
    id: 'keep_current',
    title: 'Keep current context',
    description:
      'Freeze the current canonical SKILL.md body into each harness file as a real file. Every CLI LLM keeps working with your K2-evolved context — frozen at this moment in time.',
  },
  {
    id: 'restore_original',
    title: 'Restore pre-K2 state',
    description:
      'Replace each file K2 took over with its archive from .k2so/migration/. Files K2 created fresh are removed. Your PROJECT.md / AGENT.md / archives in .k2so/ stay — reconnect later and K2 picks up where it left off.',
  },
]

export default function RemoveWorkspaceDialog(): React.JSX.Element | null {
  const isOpen = useRemoveWorkspaceDialogStore((s) => s.isOpen)
  const isPending = useRemoveWorkspaceDialogStore((s) => s.isPending)
  const projectId = useRemoveWorkspaceDialogStore((s) => s.projectId)
  const projectName = useRemoveWorkspaceDialogStore((s) => s.projectName)
  const projectPath = useRemoveWorkspaceDialogStore((s) => s.projectPath)
  const results = useRemoveWorkspaceDialogStore((s) => s.results)
  const error = useRemoveWorkspaceDialogStore((s) => s.error)
  const close = useRemoveWorkspaceDialogStore((s) => s.close)
  const setIsPending = useRemoveWorkspaceDialogStore((s) => s.setIsPending)
  const setResults = useRemoveWorkspaceDialogStore((s) => s.setResults)
  const setError = useRemoveWorkspaceDialogStore((s) => s.setError)
  const removeProject = useProjectsStore((s) => s.removeProject)

  const [mode, setMode] = useState<RemoveWorkspaceMode>('deregister_only')

  // Reset mode when dialog reopens
  useEffect(() => {
    if (isOpen) {
      setMode('deregister_only')
    }
  }, [isOpen])

  // Close on Escape (unless pending or showing results)
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !isPending) close()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, isPending, close])

  const handleConfirm = async (): Promise<void> => {
    if (!projectId || !projectPath) return
    setIsPending(true)
    try {
      let teardownResults: RemoveWorkspaceResult[] = []
      if (mode !== 'deregister_only') {
        teardownResults = await invoke<RemoveWorkspaceResult[]>('k2so_agents_teardown_workspace', {
          projectPath,
          mode,
        })
        setResults(teardownResults)
      }
      await removeProject(projectId)
      // If we showed results (keep_current / restore_original), leave them on screen briefly.
      // For deregister-only, close immediately since there's nothing to report.
      if (mode === 'deregister_only') {
        close()
      } else {
        // Switch to "results shown" state — user reads and dismisses.
        setIsPending(false)
      }
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  if (!isOpen || !projectId || !projectName) return null

  const showingResults = results.length > 0

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={isPending ? undefined : close}
    >
      <div
        className="w-[560px] max-h-[85vh] flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-2">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            {showingResults ? 'Workspace disconnected' : 'Remove Workspace'}
          </h2>
          <p className="text-[11px] text-[var(--color-text-secondary)] mt-1">
            <span className="text-[var(--color-text-primary)] font-medium">{projectName}</span>
          </p>
          {projectPath ? (
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 break-all font-mono">
              {projectPath}
            </p>
          ) : null}
        </div>

        {/* Mode picker (only before the teardown runs) */}
        {!showingResults ? (
          <div className="flex-1 overflow-y-auto px-5 pb-3">
            <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-2">
              What should K2 do with the files?
            </div>
            <div className="flex flex-col gap-2">
              {MODE_OPTIONS.map((opt) => (
                <label
                  key={opt.id}
                  className={`flex gap-2.5 p-2.5 border cursor-pointer transition-colors ${
                    mode === opt.id
                      ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/10'
                      : 'border-[var(--color-border)] bg-[var(--color-bg)]/30 hover:bg-[var(--color-bg)]/50'
                  }`}
                >
                  <input
                    type="radio"
                    name="teardown-mode"
                    value={opt.id}
                    checked={mode === opt.id}
                    onChange={() => setMode(opt.id)}
                    disabled={isPending}
                    className="mt-0.5"
                    style={{ accentColor: 'var(--color-accent)' }}
                  />
                  <div className="flex-1 min-w-0">
                    <div className="text-[11px] font-medium text-[var(--color-text-primary)]">
                      {opt.title}
                    </div>
                    <div className="text-[10px] text-[var(--color-text-muted)] leading-snug mt-0.5">
                      {opt.description}
                    </div>
                  </div>
                </label>
              ))}
            </div>
            <div className="mt-3 text-[10px] text-[var(--color-text-muted)] leading-snug">
              <span className="text-[var(--color-text-secondary)]">Nothing is destroyed.</span>{' '}
              <span className="font-mono">.k2so/</span> (including archives) is always preserved, so you can
              reconnect later and resume where you left off.
            </div>
          </div>
        ) : (
          <div className="flex-1 overflow-y-auto px-5 pb-3">
            <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1.5">
              What K2 did
            </div>
            <div className="flex flex-col gap-1">
              {results.map((r) => (
                <div
                  key={`${r.action}-${r.path}`}
                  className="flex items-start gap-2 py-1.5 px-2 border border-[var(--color-border)] bg-[var(--color-bg)]/30"
                >
                  <div className="px-1.5 py-0.5 text-[9px] uppercase tracking-wider border border-sky-500/30 bg-sky-500/10 text-sky-200 whitespace-nowrap">
                    {r.action}
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                      {r.path}
                    </div>
                    <div className="text-[10px] text-[var(--color-text-muted)] leading-snug">
                      {r.note}
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Error */}
        {error ? (
          <div className="px-5 pb-4">
            <div className="border border-red-500/30 bg-red-500/10 px-3 py-2">
              <p className="text-[11px] text-red-400 whitespace-pre-wrap">{error}</p>
            </div>
          </div>
        ) : null}

        {/* Actions */}
        <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
          {!showingResults ? (
            <>
              <button
                className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40"
                onClick={close}
                disabled={isPending}
              >
                Cancel
              </button>
              <button
                className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
                onClick={handleConfirm}
                disabled={isPending}
              >
                {isPending
                  ? 'Working…'
                  : mode === 'deregister_only'
                    ? 'Remove workspace'
                    : mode === 'keep_current'
                      ? 'Freeze context & remove'
                      : 'Restore originals & remove'}
              </button>
            </>
          ) : (
            <button
              className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)]"
              onClick={close}
            >
              Done
            </button>
          )}
        </div>
      </div>
    </div>
  )
}

// Sidebar right-click → "Rename agent…". Display name only; the handle is
// shown read-only and never changes here. See stores/rename-agent-dialog.ts.

import React, { useEffect, useRef, useState } from 'react'
import { Callout, DialogScrim } from '@/components/ui'
import { useRenameAgentDialogStore, validateAgentDisplayName } from '@/stores/rename-agent-dialog'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { agentHandle } from '@/lib/workspace-agent'
import { primaryScope } from '@/kessel/server-scope'

export default function RenameAgentDialog(): React.JSX.Element | null {
  const isOpen = useRenameAgentDialogStore((s) => s.isOpen)
  const projectId = useRenameAgentDialogStore((s) => s.projectId)
  const projectPath = useRenameAgentDialogStore((s) => s.projectPath)
  const currentName = useRenameAgentDialogStore((s) => s.currentName)
  const storedHandle = useRenameAgentDialogStore((s) => s.handle)
  const close = useRenameAgentDialogStore((s) => s.close)

  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [handle, setHandle] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  // Reset on every open; select the current name so typing replaces it.
  useEffect(() => {
    if (!isOpen) return
    setDraft(currentName)
    setError(null)
    setBusy(false)
    setHandle(storedHandle)
    requestAnimationFrame(() => inputRef.current?.select())
  }, [isOpen, currentName, storedHandle])

  // A row whose handle is not in the store yet: ask the daemon (read only).
  useEffect(() => {
    if (!isOpen || storedHandle || !projectPath) return
    let cancelled = false
    agentHandle(primaryScope(), projectPath)
      .then((h) => { if (!cancelled) setHandle(h) })
      .catch((e) => console.warn('[rename-agent] handle read failed:', e))
    return () => { cancelled = true }
  }, [isOpen, storedHandle, projectPath])

  // Esc cancels from anywhere in the dialog (not while a save is in flight).
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !busy) { e.preventDefault(); close() }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [isOpen, busy, close])

  if (!isOpen || !projectId) return null

  const cancel = (): void => {
    if (!busy) close()
  }

  const save = async (): Promise<void> => {
    if (busy) return
    const v = validateAgentDisplayName(draft)
    if ('error' in v) { setError(v.error); return }
    if (v.name === currentName) { close(); return }
    setError(null)
    setBusy(true)
    try {
      await useProjectsStore.getState().renameAgentDisplayName(projectId, v.name)
      close()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  const openWorkspaceSettings = (): void => {
    close()
    useSettingsStore.getState().openSettings('projects', projectId)
  }

  const valid = !('error' in validateAgentDisplayName(draft))

  return (
    <DialogScrim
      zIndex={50}
      className="flex items-center justify-center no-drag"
      style={{ backdropFilter: 'blur(4px)' }}
      onClick={cancel}
    >
      <div
        role="dialog"
        aria-label="Rename agent"
        className="w-[420px] flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-5 pt-5 pb-3">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Rename agent</h2>
          <label
            htmlFor="rename-agent-name"
            className="block text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mt-3 mb-1"
          >
            Agent Name
          </label>
          <input
            id="rename-agent-name"
            ref={inputRef}
            type="text"
            value={draft}
            autoFocus
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            onChange={(e) => { setDraft(e.target.value); setError(null) }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') { e.preventDefault(); void save() }
            }}
            className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-60"
          />
          <p data-testid="rename-agent-handle" className="text-[10px] text-[var(--color-text-muted)] mt-1.5 leading-snug">
            Handle stays{' '}
            <span className="font-mono text-[var(--color-text-secondary)]">@{handle || '…'}</span>
            ; change it in{' '}
            <button
              type="button"
              onClick={openWorkspaceSettings}
              className="underline hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
            >
              workspace settings
            </button>
            .
          </p>
        </div>

        {error ? (
          <div className="px-5 pb-3">
            <Callout tone="error">
              <p className="whitespace-pre-wrap">{error}</p>
            </Callout>
          </div>
        ) : null}

        <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
          <button
            type="button"
            className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40"
            onClick={cancel}
            disabled={busy}
          >
            Cancel
          </button>
          <button
            type="button"
            className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
            onClick={() => void save()}
            disabled={busy || !valid}
          >
            {busy ? 'Saving…' : 'Save'}
          </button>
        </div>
      </div>
    </DialogScrim>
  )
}

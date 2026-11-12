import React, { useEffect, useState } from 'react'
import { floatMaterial } from '@/components/ui/float-material'
import { Callout, DialogScrim, Checkbox, DialogCloseButton, useDialogCancel } from '@/components/ui'
import { useAddWorkspaceDialogStore } from '../../stores/add-workspace-dialog'
import { GitToolsBanner } from '@/components/GitTools/GitToolsNotice'
import { ADD_WORKSPACE_Z } from '@/lib/overlay-layers'

// The Skip / Start-Fresh / Adopt consent picker was removed with the
// canonical-agents PRD (§7 Removal Catalog): harness fan-out is now off by
// default, so adding a workspace no longer touches any harness file and
// needs no consent gate. The value-pitch WHY copy ("Tell K2 once, every
// AI tool listens…") moved verbatim into the K2 Canonical Agent skill
// briefing + the canonical button subtitle in the Agent section — that is
// where unification is actually opt-in. This dialog is now a plain confirm.
export default function AddWorkspaceDialog(): React.JSX.Element | null {
  const isOpen = useAddWorkspaceDialogStore((s) => s.isOpen)
  const isPending = useAddWorkspaceDialogStore((s) => s.isPending)
  const path = useAddWorkspaceDialogStore((s) => s.path)
  const error = useAddWorkspaceDialogStore((s) => s.error)
  const onConfirm = useAddWorkspaceDialogStore((s) => s.onConfirm)
  const close = useAddWorkspaceDialogStore((s) => s.close)
  const setIsPending = useAddWorkspaceDialogStore((s) => s.setIsPending)
  const setError = useAddWorkspaceDialogStore((s) => s.setError)
  const [seedWiki, setSeedWiki] = useState(true)
  const [seedAgentsMd, setSeedAgentsMd] = useState(true)
  const [fanout, setFanout] = useState(false)

  useEffect(() => {
    if (isOpen) {
      setSeedWiki(true)
      setSeedAgentsMd(true)
      setFanout(false)
    }
  }, [isOpen])

  // Esc, the × and a press outside cancel; held open while adding.
  const cancel = useDialogCancel(isOpen ? close : undefined, isOpen && isPending)

  const handleAdd = async (): Promise<void> => {
    if (!onConfirm) return
    setIsPending(true)
    try {
      await onConfirm({ seedWiki, seedAgentsMd, fanout })
      close()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
      setIsPending(false)
    }
  }

  if (!isOpen || !path) return null

  return (
    <DialogScrim
      zIndex={ADD_WORKSPACE_Z}
      className="flex items-center justify-center no-drag"
      style={{ backdropFilter: 'blur(4px)' }}
    >
      <div
        {...floatMaterial('dialog')} className="w-[520px] max-h-[calc(var(--k2-vh)*0.85)] flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        {...cancel}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-2">
          <div className="flex items-start justify-between gap-2">
            <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
              Add Workspace to K2
            </h2>
            <DialogCloseButton className="-mt-3 -mr-3" onClick={close} disabled={isPending} heldReason="Can’t close while the workspace is being added" />
          </div>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1 break-all font-mono">
            {path}
          </p>
        </div>

        {/* 0.46.4: no git on the server yet (a Mac without Apple's command
            line tools). The workspace is still added; the daemon does its
            git steps once git runs. Renders nothing while git works. */}
        <div className="px-5 pb-3 empty:hidden">
          <GitToolsBanner />
        </div>

        {/* Generate adds cwd AGENTS.md when missing; it will not replace a user file.
            Leftover fan-out can replace CLAUDE.md / GEMINI.md / … */}
        <div className="px-5 pb-3">
          <div className="border border-[var(--color-border)] bg-[var(--color-bg)]/40 px-3 py-3 text-[11px] leading-relaxed text-[var(--color-text-secondary)] space-y-2">
            <p>
              Generate will <span className="text-[var(--color-text-primary)]">add</span>{' '}
              <span className="font-mono">AGENTS.md</span> when that file is missing. It will
              not replace a file you already wrote. Leftover fan-out (off by default){' '}
              <span className="text-[var(--color-text-primary)]">can replace</span>{' '}
              <span className="font-mono">CLAUDE.md</span>,{' '}
              <span className="font-mono">GEMINI.md</span>, and other harness names.
            </p>
          </div>
        </div>

        <div className="px-5 pb-3">
          <label className="flex items-start gap-2 cursor-pointer select-none no-drag">
            <Checkbox
              checked={seedWiki}
              onChange={(e) => setSeedWiki(e.target.checked)}
              disabled={isPending}
              className="mt-0.5"
            />
            <span className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
              Seed the wiki
              <span className="block text-[10px] text-[var(--color-text-muted)] mt-0.5">
                Creates <span className="font-mono">Home.md</span> and{' '}
                <span className="font-mono">_Index.md</span> under{' '}
                <span className="font-mono">.k2/wiki/</span>. Uncheck to skip.
              </span>
            </span>
          </label>
          <label className="flex items-start gap-2 cursor-pointer select-none no-drag mt-2">
            <Checkbox
              checked={seedAgentsMd}
              onChange={(e) => setSeedAgentsMd(e.target.checked)}
              disabled={isPending}
              className="mt-0.5"
            />
            <span className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
              Write cwd AGENTS.md
              <span className="block text-[10px] text-[var(--color-text-muted)] mt-0.5">
                The file most AI tools load. Uncheck to keep it only under{' '}
                <span className="font-mono">.k2/</span>. A user-authored file is never overwritten.
              </span>
            </span>
          </label>
          <label className="flex items-start gap-2 cursor-pointer select-none no-drag mt-2">
            <Checkbox
              checked={fanout}
              onChange={(e) => setFanout(e.target.checked)}
              disabled={isPending}
              className="mt-0.5"
            />
            <span className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
              Also symlink leftover harness files
              <span className="block text-[10px] text-[var(--color-text-muted)] mt-0.5">
                <span className="font-mono">CLAUDE.md</span>,{' '}
                <span className="font-mono">GEMINI.md</span>,{' '}
                <span className="font-mono">AGENT.md</span>, … →{' '}
                <span className="font-mono">AGENTS.md</span>. Some tools still look for those names.
                Existing leftover files can be replaced.
              </span>
            </span>
          </label>
        </div>

        {/* Error */}
        {error ? (
          <div className="px-5 pb-4">
            <Callout tone="error">
              <p className="whitespace-pre-wrap">{error}</p>
            </Callout>
          </div>
        ) : null}

        {/* Actions */}
        <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
          <button
            className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40"
            onClick={close}
            disabled={isPending}
          >
            Cancel
          </button>
          <button
            className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
            onClick={handleAdd}
            disabled={isPending}
          >
            {isPending ? 'Adding…' : 'Add workspace'}
          </button>
        </div>
      </div>
    </DialogScrim>
  )
}

import React, { useEffect } from 'react'

import { useCloneToDialogStore } from '../../stores/clone-to-dialog'
import type { CloneStage } from '../../lib/clone-to'

// Progress modal for "Clone to" (K2 Connect workspace migration). Driven
// entirely by the clone-to-dialog store, which the orchestration's hooks
// update. Visual style mirrors AddWorkspaceDialog (CSS vars, same modal
// chrome). The modal stays open through 'done' / 'error' so the user reads
// the result + the re-supply checklist, then closes it manually.

// Ordered stage list for the little step tracker. 'done' / 'error' are
// terminal and rendered separately.
const STEPS: { stage: CloneStage; label: string }[] = [
  { stage: 'bundling', label: 'Bundling workspace' },
  { stage: 'connecting', label: 'Connecting to host' },
  { stage: 'choosing-folder', label: 'Choosing destination folder' },
  { stage: 'uploading', label: 'Uploading bundle' },
  { stage: 'unpacking', label: 'Unpacking on host' },
]

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

export default function CloneToDialog(): React.JSX.Element | null {
  const isOpen = useCloneToDialogStore((s) => s.isOpen)
  const phase = useCloneToDialogStore((s) => s.phase)
  const projectName = useCloneToDialogStore((s) => s.projectName)
  const host = useCloneToDialogStore((s) => s.host)
  const carrySecrets = useCloneToDialogStore((s) => s.carrySecrets)
  const setCarrySecrets = useCloneToDialogStore((s) => s.setCarrySecrets)
  const includeAllHistory = useCloneToDialogStore((s) => s.includeAllHistory)
  const setIncludeAllHistory = useCloneToDialogStore((s) => s.setIncludeAllHistory)
  const confirm = useCloneToDialogStore((s) => s.confirm)
  const stage = useCloneToDialogStore((s) => s.stage)
  const summary = useCloneToDialogStore((s) => s.summary)
  const result = useCloneToDialogStore((s) => s.result)
  const error = useCloneToDialogStore((s) => s.error)
  const close = useCloneToDialogStore((s) => s.close)
  const openResult = useCloneToDialogStore((s) => s.openResult)

  const isOptions = phase === 'options'
  const isTerminal = stage === 'done' || stage === 'error'
  // Closable (via backdrop / Esc) on the options panel (it's a no-op cancel)
  // and once the run is terminal — never mid-flight (closing wouldn't cancel
  // the in-progress daemon work, so we don't pretend it does).
  const isClosable = isOptions || isTerminal

  // Esc closes on the options panel or once terminal. Enter on the options
  // panel proceeds (convenience — matches the primary Clone button).
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && isClosable) close()
      else if (e.key === 'Enter' && isOptions) confirm()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, isClosable, isOptions, close, confirm])

  if (!isOpen) return null

  const activeIdx = STEPS.findIndex((s) => s.stage === stage)

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={isClosable ? close : undefined}
    >
      <div
        className="w-[560px] max-h-[85vh] flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-2">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            {isOptions
              ? 'Clone workspace'
              : stage === 'done'
                ? 'Workspace cloned'
                : stage === 'error'
                  ? 'Clone failed'
                  : 'Cloning workspace'}
          </h2>
          <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
            <span className="text-[var(--color-text-secondary)]">{projectName ?? 'workspace'}</span>
            {host ? (
              <>
                {' → '}
                <span className="text-[var(--color-text-secondary)]">{host.label}</span>
              </>
            ) : null}
          </p>
        </div>

        {/* Body */}
        <div className="px-5 pb-3 overflow-y-auto">
          {/* Pre-flight options panel — the "Include secrets" toggle. Shown
              before any orchestration runs; clicking Clone flips to the
              progress phase. */}
          {isOptions && (
            <div className="space-y-3 pt-1">
              <label className="flex items-start gap-2 cursor-pointer select-none no-drag">
                <input
                  type="checkbox"
                  checked={carrySecrets}
                  onChange={(e) => setCarrySecrets(e.target.checked)}
                  className="peer sr-only"
                />
                <span
                  aria-hidden="true"
                  className="mt-0.5 w-3.5 h-3.5 flex-shrink-0 flex items-center justify-center border transition-colors border-[var(--color-border)] bg-[var(--color-bg-elevated)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)] peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)]"
                >
                  {carrySecrets && (
                    <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                    </svg>
                  )}
                </span>
                <span className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
                  Include secrets (<span className="font-mono text-[11px]">.env</span>,{' '}
                  <span className="font-mono text-[11px]">.auth/</span>, in-workspace tokens)
                </span>
              </label>
              <p className="text-[11px] text-[var(--color-text-muted)] leading-relaxed pl-[22px]">
                Carried securely over the encrypted connection between your machines.
                Uncheck to scrub them — you’ll re-add them on the host.
              </p>

              <label className="flex items-start gap-2 cursor-pointer select-none no-drag">
                <input
                  type="checkbox"
                  checked={includeAllHistory}
                  onChange={(e) => setIncludeAllHistory(e.target.checked)}
                  className="peer sr-only"
                />
                <span
                  aria-hidden="true"
                  className="mt-0.5 w-3.5 h-3.5 flex-shrink-0 flex items-center justify-center border transition-colors border-[var(--color-border)] bg-[var(--color-bg-elevated)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)] peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)]"
                >
                  {includeAllHistory && (
                    <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                    </svg>
                  )}
                </span>
                <span className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
                  Include all chat history
                </span>
              </label>
              <p className="text-[11px] text-[var(--color-text-muted)] leading-relaxed pl-[22px]">
                Migrates every Claude Code session transcript for this workspace.
                Uncheck to carry only the most recent (live) session.
              </p>
            </div>
          )}

          {/* Step tracker — hidden on the options + error screens (the error
              screen shows the failure + which step it reached via the
              message). */}
          {!isOptions && stage !== 'error' && (
            <div className="border border-[var(--color-border)] bg-[var(--color-bg)]/40 px-3 py-3 space-y-1.5">
              {STEPS.map((step, i) => {
                const isCurrent = i === activeIdx && stage !== 'done'
                const isComplete = stage === 'done' || i < activeIdx
                return (
                  <div key={step.stage} className="flex items-center gap-2 text-[11px]">
                    <span
                      className="inline-flex items-center justify-center w-3.5 h-3.5"
                      aria-hidden
                    >
                      {isComplete ? (
                        <svg className="w-3 h-3 text-green-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={3} strokeLinecap="round" strokeLinejoin="round">
                          <polyline points="20 6 9 17 4 12" />
                        </svg>
                      ) : isCurrent ? (
                        <span className="w-2 h-2 rounded-full bg-[var(--color-text-primary)] animate-pulse" />
                      ) : (
                        <span className="w-2 h-2 rounded-full border border-[var(--color-border)]" />
                      )}
                    </span>
                    <span
                      className={
                        isComplete
                          ? 'text-[var(--color-text-secondary)]'
                          : isCurrent
                            ? 'text-[var(--color-text-primary)]'
                            : 'text-[var(--color-text-muted)]'
                      }
                    >
                      {step.label}
                    </span>
                  </div>
                )
              })}
            </div>
          )}

          {/* Manifest summary — shown once the bundle is built. */}
          {summary && stage !== 'error' && (
            <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1 text-[10px] text-[var(--color-text-muted)]">
              <span>{summary.entry_count} files</span>
              <span>{formatBytes(summary.size_bytes)}</span>
              <span>{summary.scrubbed_secret_count} secret(s) scrubbed</span>
            </div>
          )}

          {/* Done screen — destination + re-supply checklist. */}
          {stage === 'done' && result && (
            <div className="mt-3 space-y-3">
              <div>
                <p className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)] mb-1">
                  Landed at
                </p>
                <p className="text-[11px] font-mono text-[var(--color-text-secondary)] break-all">
                  {result.dest_path}
                </p>
              </div>

              <div className="border border-[var(--color-border)] bg-[var(--color-bg)]/40 px-3 py-3">
                <p className="text-[11px] font-medium text-[var(--color-text-primary)] mb-1.5">
                  Re-supply checklist
                </p>
                {/* Scrubbed secrets by relative path, IF the backend
                    surfaces them; today's manifest_summary only carries the
                    COUNT, so we note the per-path list as a follow-up. */}
                {summary?.scrubbed_secrets && summary.scrubbed_secrets.length > 0 ? (
                  <div className="mb-2">
                    <p className="text-[10px] text-[var(--color-text-muted)] mb-1">
                      Secrets scrubbed — re-supply these on the host:
                    </p>
                    <ul className="list-disc list-inside space-y-0.5">
                      {summary.scrubbed_secrets.map((p) => (
                        <li key={p} className="text-[10px] font-mono text-[var(--color-text-secondary)] break-all">
                          {p}
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : summary && summary.scrubbed_secret_count > 0 ? (
                  <p className="text-[10px] text-[var(--color-text-muted)] mb-2">
                    {summary.scrubbed_secret_count} secret file(s) were scrubbed from the bundle.
                    Re-supply them on the host (the per-file list isn’t returned yet — open the
                    cloned workspace and re-add your <span className="font-mono">.env</span> /
                    auth files).
                  </p>
                ) : null}
                <ul className="list-disc list-inside space-y-0.5 text-[10px] text-[var(--color-text-secondary)]">
                  <li>Re-authenticate any MCP servers (creds/config aren’t carried).</li>
                  <li>Re-grant OS-permission-gated tooling (e.g. Full Disk Access / Automation).</li>
                  <li>The host needs its own Claude Code auth — sign in there.</li>
                </ul>
              </div>
            </div>
          )}

          {/* Error screen. */}
          {stage === 'error' && error && (
            <div className="border border-red-500/30 bg-red-500/10 px-3 py-2">
              <p className="text-[11px] text-red-400 whitespace-pre-wrap break-words">{error}</p>
            </div>
          )}
        </div>

        {/* Actions */}
        <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
          {isOptions ? (
            <>
              <button
                className="px-3 py-1.5 text-xs font-medium text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)]"
                onClick={close}
              >
                Cancel
              </button>
              <button
                className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)]"
                onClick={confirm}
              >
                Clone
              </button>
            </>
          ) : (
            <>
              {/* On success, offer to jump straight into the freshly-cloned
                  workspace on the (still-active) destination host. */}
              {stage === 'done' && result?.project?.id && host && (
                <button
                  className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)]"
                  onClick={() => void openResult()}
                >
                  Open on {host.label}
                </button>
              )}
              <button
                className="px-3 py-1.5 text-xs font-medium text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] disabled:opacity-40"
                onClick={close}
                disabled={!isTerminal}
              >
                {isTerminal ? 'Close' : 'Working…'}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  )
}

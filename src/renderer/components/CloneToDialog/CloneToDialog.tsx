import React, { useEffect } from 'react'

import { Button, Callout, DialogFrame, DialogScrim } from '@/components/ui'
import { useCloneToDialogStore } from '../../stores/clone-to-dialog'
import type { CloneStage } from '../../lib/clone-to'
import {
  cloneButtonLabel,
  doneCountsLine,
  EnvFilesBlock,
  PasswordPromptBlock,
  SecretsBlock,
  ThreadBlock,
  Tick,
} from './CloneOptionsBlocks'

// Progress modal for "Clone to" (K2 Connect workspace migration). Driven
// entirely by the clone-to-dialog store, which the orchestration's hooks
// update. Visual style mirrors AddWorkspaceDialog (CSS vars, same modal
// chrome). The modal stays open through 'done' / 'error' so the user reads
// the result + the re-supply checklist, then closes it manually.

// Ordered stage lists for the little step tracker. 'done' / 'error' are
// terminal and rendered separately. PUSH is the original local→host
// migration; PULL is "Clone to this computer" (0.40.22 — remote→local).
const PUSH_STEPS: { stage: CloneStage; label: string }[] = [
  { stage: 'bundling', label: 'Bundling workspace' },
  { stage: 'connecting', label: 'Connecting to host' },
  { stage: 'choosing-folder', label: 'Choosing destination folder' },
  { stage: 'uploading', label: 'Uploading bundle' },
  { stage: 'unpacking', label: 'Unpacking on host' },
]
const PULL_STEPS: { stage: CloneStage; label: string }[] = [
  { stage: 'packing', label: 'Packing workspace on the server' },
  { stage: 'choosing-folder', label: 'Choosing destination folder' },
  { stage: 'downloading', label: 'Downloading bundle' },
  { stage: 'unpacking', label: 'Unpacking on this computer' },
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
  const pull = useCloneToDialogStore((s) => s.pull)
  const carrySecrets = useCloneToDialogStore((s) => s.carrySecrets)
  const secretChoices = useCloneToDialogStore((s) => s.secretChoices)
  const confirming = useCloneToDialogStore((s) => s.confirming)
  const optionsError = useCloneToDialogStore((s) => s.optionsError)
  const previewError = useCloneToDialogStore((s) => s.previewError)
  const previewLoading = useCloneToDialogStore((s) => s.previewLoading)
  const includeAllHistory = useCloneToDialogStore((s) => s.includeAllHistory)
  const setIncludeAllHistory = useCloneToDialogStore((s) => s.setIncludeAllHistory)
  const confirm = useCloneToDialogStore((s) => s.confirm)
  const stage = useCloneToDialogStore((s) => s.stage)
  const summary = useCloneToDialogStore((s) => s.summary)
  const result = useCloneToDialogStore((s) => s.result)
  const vaultOutcome = useCloneToDialogStore((s) => s.vaultOutcome)
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
      else if (e.key === 'Enter' && isOptions) void confirm()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, isClosable, isOptions, close, confirm])

  if (!isOpen) return null

  const steps = pull ? PULL_STEPS : PUSH_STEPS
  // Destination shown in the header: the K2 Connect host for a push,
  // "This computer" for a pull.
  const destLabel = host?.label ?? (pull ? 'This computer' : null)
  const activeIdx = steps.findIndex((s) => s.stage === stage)

  return (
    <>
      <DialogScrim
        zIndex={50}
        className="no-drag"
        style={{ backdropFilter: 'blur(4px)' }}
        onClick={isClosable ? close : undefined}
        data-testid="clone-dialog-scrim"
      />
      <DialogFrame
        zIndex={51}
        className="w-[560px] max-w-[calc(100vw-32px)] max-h-[85vh] flex flex-col"
        data-testid="clone-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Clone workspace"
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
            {destLabel ? (
              <>
                {' → '}
                <span className="text-[var(--color-text-secondary)]">{destLabel}</span>
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
              {/* 0.46.0: Thread, per-secret step, `.env` files (D3/D1/D4). */}
              <ThreadBlock />
              {previewLoading && (
                <p className="text-[11px] text-[var(--color-text-muted)]">Checking what travels…</p>
              )}
              {previewError && (
                <Callout tone="error">
                  <p className="whitespace-pre-wrap break-words" data-testid="clone-preview-error">{previewError}</p>
                </Callout>
              )}
              <SecretsBlock pull={pull} />
              <EnvFilesBlock pull={pull} />

              <Tick checked={includeAllHistory} onChange={setIncludeAllHistory} testId="clone-all-history">
                Include all chat history
              </Tick>
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
              {steps.map((step, i) => {
                const isCurrent = i === activeIdx && stage !== 'done'
                const isComplete = stage === 'done' || i < activeIdx
                return (
                  <div key={step.stage} className="flex items-center gap-2 text-[11px]">
                    <span
                      className="inline-flex items-center justify-center w-3.5 h-3.5"
                      aria-hidden
                    >
                      {isComplete ? (
                        <svg className="w-3 h-3 text-[var(--color-status-ok-soft)]" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={3} strokeLinecap="round" strokeLinejoin="round">
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

              {doneCountsLine(result, vaultOutcome).length > 0 && (
                <div className="space-y-0.5" data-testid="clone-done-counts">
                  {doneCountsLine(result, vaultOutcome).map((line) => (
                    <p key={line} className="text-[11px] text-[var(--color-text-secondary)]">{line}</p>
                  ))}
                </div>
              )}

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
                      Secrets scrubbed — re-supply these on {pull ? 'this computer' : 'the host'}:
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
                    Re-supply them on {pull ? 'this computer' : 'the host'} (the per-file list
                    isn’t returned yet — open the cloned workspace and re-add your{' '}
                    <span className="font-mono">.env</span> / auth files).
                  </p>
                ) : null}
                <ul className="list-disc list-inside space-y-0.5 text-[10px] text-[var(--color-text-secondary)]">
                  <li>Re-authenticate any MCP servers (creds/config aren’t carried).</li>
                  <li>Re-grant OS-permission-gated tooling (e.g. Full Disk Access / Automation).</li>
                  <li>
                    {pull
                      ? 'The cloned workspace uses this computer’s Claude Code auth — sign in here if you haven’t.'
                      : 'The host needs its own Claude Code auth — sign in there.'}
                  </li>
                </ul>
              </div>
            </div>
          )}

          {isOptions && (
            <div className="mt-3 space-y-2">
              <PasswordPromptBlock />
              {optionsError && (
                <Callout tone="error">
                  <p className="whitespace-pre-wrap break-words" data-testid="clone-options-error">{optionsError}</p>
                </Callout>
              )}
            </div>
          )}

          {/* Error screen. */}
          {stage === 'error' && error && (
            <Callout tone="error">
              <p className="whitespace-pre-wrap break-words">{error}</p>
            </Callout>
          )}
        </div>

        {/* Actions */}
        <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
          {isOptions ? (
            <>
              <Button variant="ghost" size="md" onClick={close}>
                Cancel
              </Button>
              <Button
                variant="accent"
                size="md"
                onClick={() => void confirm()}
                disabled={confirming}
                data-testid="clone-confirm-button"
              >
                {cloneButtonLabel(secretChoices, carrySecrets, confirming)}
              </Button>
            </>
          ) : (
            <>
              {/* On success, offer to jump straight into the freshly-cloned
                  workspace on the (still-active) destination host. */}
              {stage === 'done' && result?.project?.id && host && (
                <Button variant="accent" size="md" onClick={() => void openResult()}>
                  Open on {host.label}
                </Button>
              )}
              <Button variant="ghost" size="md" onClick={close} disabled={!isTerminal}>
                {isTerminal ? 'Close' : 'Working…'}
              </Button>
            </>
          )}
        </div>
      </DialogFrame>
    </>
  )
}

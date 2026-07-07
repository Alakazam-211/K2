import React from 'react'
import { useEffect, useState } from 'react'
import { daemonCliPost } from '@/lib/daemon-cli'
import { AgentContextDiagram } from './AgentContextDiagram'
import { FANOUT_ENABLE_WARNING } from './CanonicalAgentButtons'

// Shared confirmation modal for the per-workspace "Canonical Agent" /
// "Enable harness fan-out" checkbox. Replaces the bare `window.confirm`
// that used to gate the destructive programmatic-symlink route.
//
// Why a real modal: enabling fan-out symlinks the harness files
// (CLAUDE.md, AGENTS.md, …) onto K2's generated canon and CAN OVERWRITE
// existing content. A one-line `confirm()` couldn't show the user WHAT
// canonical fan-out sets up, nor offer the safe alternative. This modal:
//   • renders the AgentContextDiagram so the user SEES AGENT.md → mirrors,
//   • explains the overwrite risk using the canonical FANOUT_ENABLE_WARNING,
//   • offers THREE actions:
//       1. "Set it up agentically" (recommended/safe) — the Skill route
//          (PRD §11). Writes the `k2-canonical-agent` opt-in skill to this
//          workspace via the existing `POST /cli/skills/write-opt-in` route
//          (the same route RoleSkillEditor uses). This MERGES content
//          rather than overwriting and does NOT enable programmatic fan-out.
//          The user then runs the K2 Canonical Agent from the workspace's
//          Agent chat to complete it.
//       2. "Continue" (programmatic symlinks) — the current behavior. POSTs
//          `onboarding/set-harness-fanout-enabled` (marker write + regen).
//          Styled as the more-cautionary option because it can overwrite.
//       3. "Cancel" — close, change nothing; the caller reverts the box.

export interface FanoutConfirmModalProps {
  /** Active workspace path the action applies to. */
  projectPath: string
  /** Close the modal WITHOUT taking either apply action (Cancel / overlay /
   *  Escape). The caller must revert the checkbox to unchecked. */
  onCancel: () => void
  /** The user picked "Continue" and the programmatic fan-out marker was
   *  written successfully — the caller should reflect the box as enabled. */
  onProgrammaticEnabled: () => void
  /** The user picked "Set it up agentically" and the canonical-agent skill
   *  was written successfully. Programmatic fan-out was NOT enabled. The
   *  caller decides the resulting checkbox state (we leave it unchecked —
   *  the skill route does not flip the fan-out marker). */
  onSkillRouteTaken: () => void
}

type Busy = 'skill' | 'programmatic' | null

export function FanoutConfirmModal({
  projectPath,
  onCancel,
  onProgrammaticEnabled,
  onSkillRouteTaken,
}: FanoutConfirmModalProps): React.JSX.Element {
  const [busy, setBusy] = useState<Busy>(null)
  const [error, setError] = useState<string | null>(null)

  // Escape closes (== Cancel) unless an action is mid-flight.
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && busy === null) onCancel()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [busy, onCancel])

  // Recommended/safe: enable the K2 Canonical Agent skill for this
  // workspace (merge route). Does NOT touch the fan-out marker.
  async function handleAgentic(): Promise<void> {
    if (busy !== null) return
    setBusy('skill')
    setError(null)
    try {
      await daemonCliPost('skills/write-opt-in', {
        project_path: projectPath,
        skill: 'k2-canonical-agent',
      })
      onSkillRouteTaken()
    } catch (err) {
      console.error('[fanout-modal] write-opt-in k2-canonical-agent failed:', err)
      setError('Could not set up the K2 Canonical Agent skill. Please try again.')
      setBusy(null)
    }
  }

  // Cautionary: the programmatic symlink route — writes the marker +
  // regenerates the harness symlinks. Can overwrite existing content.
  async function handleProgrammatic(): Promise<void> {
    if (busy !== null) return
    setBusy('programmatic')
    setError(null)
    try {
      await daemonCliPost('onboarding/set-harness-fanout-enabled', {
        project_path: projectPath,
        enabled: true,
      })
      onProgrammaticEnabled()
    } catch (err) {
      console.error('[fanout-modal] set-harness-fanout-enabled failed:', err)
      setError('Could not enable programmatic fan-out. Please try again.')
      setBusy(null)
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={busy === null ? onCancel : undefined}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Set up canonical harness fan-out"
        className="w-[820px] max-w-[92vw] max-h-[90vh] overflow-y-auto border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-2">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            Set up canonical harness fan-out
          </h2>
          <p className="text-xs text-[var(--color-text-muted)] mt-1 leading-relaxed">
            Canonical fan-out makes K2&rsquo;s generated{' '}
            <span className="font-mono text-[var(--color-text-secondary)]">.k2/AGENTS.md</span> the
            single source of truth, and points each AI tool&rsquo;s file
            (<span className="font-mono">CLAUDE.md</span>, <span className="font-mono">AGENTS.md</span>, &hellip;)
            back at it. Here&rsquo;s what that sets up:
          </p>
        </div>

        {/* The diagram — shows AGENT.md → generated canon → harness mirrors. */}
        <div className="px-5 pb-1">
          <AgentContextDiagram />
        </div>

        {/* Safety note — displaced files are moved into .k2/migration/. */}
        <div className="px-5 pb-3">
          <div className="border-l-2 border-[var(--color-accent)]/50 bg-[var(--color-accent)]/5 pl-3 pr-2 py-2 text-[11px] leading-snug text-[var(--color-text-muted)]">
            <span className="font-medium text-[var(--color-text-secondary)]">Safe by default: </span>
            {FANOUT_ENABLE_WARNING}
          </div>
        </div>

        {error && (
          <div className="px-5 pb-2">
            <p className="text-[11px] text-[var(--color-status-error-soft)]">{error}</p>
          </div>
        )}

        {/* Two clear choices side by side; Cancel below. */}
        <div className="px-5 pb-5">
          <div className="grid grid-cols-2 gap-3">
            {/* Recommended — the merge (skill) route. */}
            <button
              onClick={handleAgentic}
              disabled={busy !== null}
              className="px-4 py-3 text-left border border-[var(--color-accent)]/40 bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 transition-colors disabled:opacity-40 no-drag cursor-pointer"
            >
              <div className="text-xs font-medium text-[var(--color-accent)]">
                {busy === 'skill' ? 'Setting up…' : 'Set it up agentically'}
              </div>
              <div className="mt-1 text-[11px] leading-snug text-[var(--color-text-muted)]">
                Recommended. The K2 Canonical Agent merges your files into the canon — nothing changes without you.
              </div>
            </button>

            {/* Direct — programmatic symlinks (originals moved into .k2/migration/). */}
            <button
              onClick={handleProgrammatic}
              disabled={busy !== null}
              className="px-4 py-3 text-left border border-[var(--color-border)] bg-[var(--color-bg-elevated)] hover:bg-[var(--color-bg-surface)] transition-colors disabled:opacity-40 no-drag cursor-pointer"
            >
              <div className="text-xs font-medium text-[var(--color-text-primary)]">
                {busy === 'programmatic' ? 'Enabling…' : 'Symlink it now'}
              </div>
              <div className="mt-1 text-[11px] leading-snug text-[var(--color-text-muted)]">
                K2 links your files to the canon automatically. Your originals are moved into .k2/migration/ first.
              </div>
            </button>
          </div>

          <button
            onClick={onCancel}
            disabled={busy !== null}
            className="mt-3 w-full px-3 py-1.5 text-xs text-center text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors disabled:opacity-40 no-drag cursor-pointer"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  )
}

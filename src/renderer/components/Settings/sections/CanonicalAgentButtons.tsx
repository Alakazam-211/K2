import { useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { type RoleSkill, roleSkillLabel } from './canonicalAgentSeeds'
import { type HarnessProbe, anyHarnessUnified } from './canonicalState'
import { FanoutConfirmModal } from './FanoutConfirmModal'

// The value-pitch WHY copy relocated VERBATIM from the removed consent page
// (AddWorkspaceDialog:147-164) into the canonical button subtitle + the skill
// briefing (canonical-agents PRD §7). This is where harness unification is
// actually opt-in, so the pitch belongs here.
export const CANONICAL_PITCH_SUBTITLE =
  'Tell K2 once, every AI tool listens. Each AI coding tool reads its project notes from a different file; write your context once and every tool sees the same picture.'

// Plain-language warning shown at the fan-out checkbox decision point.
// Enabling fan-out symlinks harness files onto K2's generated canon and can
// overwrite existing content — the safe route for an existing project is the
// K2 Canonical Agent skill (it merges content first). Best for new projects.
export const FANOUT_ENABLE_WARNING =
  'Your existing leftover harness files (CLAUDE.md, GEMINI.md, AGENT.md, …) are moved into .k2/migration/ before they’re replaced with symlinks to AGENTS.md — nothing is deleted, and you can restore them straight from that folder if you change your mind. Cwd AGENTS.md is not a fan-out victim.'

/**
 * Role-skill button (Workspace Manager / K2 Agent). Opens the normal
 * AIFileEditor on AGENT.md (PRD §9.1). Label gates on skill-present state:
 * "Set up …" when no SKILL.md exists yet, "Re-run …" once it does (§9.3).
 */
export function RoleSkillButton({
  role,
  projectPath,
  onOpen,
}: {
  role: RoleSkill
  projectPath: string
  onOpen: () => void
}): React.JSX.Element {
  const label = roleSkillLabel(role)
  const [skillPresent, setSkillPresent] = useState(false)

  useEffect(() => {
    let cancelled = false
    daemonCliGet<{ content: string }>('fs/read-file', {
      path: `${projectPath}/.k2/skills/${role}/SKILL.md`,
    })
      .then(() => { if (!cancelled) setSkillPresent(true) })
      .catch(() => { if (!cancelled) setSkillPresent(false) })
    return () => { cancelled = true }
  }, [projectPath, role])

  return (
    <div className="flex items-center justify-between">
      <div className="min-w-0">
        <span className="text-xs text-[var(--color-text-secondary)]">{label} skill</span>
        <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">
          Weaves the {label} role guidance into{' '}
          <span className="font-mono">.k2/agent/AGENT.md</span> organically — your existing
          context is preserved, never overwritten with a templated block.
        </p>
      </div>
      <button
        onClick={onOpen}
        className="px-2.5 py-1 text-[11px] font-medium text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors whitespace-nowrap no-drag cursor-pointer"
      >
        {skillPresent ? `Re-run ${label}` : `Set up ${label}`}
      </button>
    </div>
  )
}

/**
 * K2 Canonical Agent control — setup ceremony + harness fan-out toggle.
 * Compact card layout for the Context tab left column (and other embeds).
 * Label gates on detect_canonical_state: "Manage / Undo" when unified,
 * "Set up" otherwise (PRD §9.3).
 */
export function CanonicalAgentButton({
  probes,
  projectPath,
  onOpen,
}: {
  probes: HarnessProbe[]
  projectPath: string
  onOpen: (mode: 'setup' | 'manage') => void
}): React.JSX.Element {
  const unified = anyHarnessUnified(probes)
  const [generateEnabled, setGenerateEnabled] = useState(false)
  const [generateBusy, setGenerateBusy] = useState(false)
  const [generateSkip, setGenerateSkip] = useState<string | null>(null)
  const [fanoutEnabled, setFanoutEnabled] = useState(false)
  const [fanoutBusy, setFanoutBusy] = useState(false)
  // Enabling opens the confirmation modal; disabling applies immediately.
  const [showFanoutModal, setShowFanoutModal] = useState(false)
  const [skillHint, setSkillHint] = useState(false)

  useEffect(() => {
    let cancelled = false
    daemonCliPost<{ enabled: boolean }>('onboarding/agents-md-generate-enabled', { project_path: projectPath })
      .then((r) => r.enabled)
      .then((on) => { if (!cancelled) setGenerateEnabled(on) })
      .catch(() => { /* default off for existing workspaces without a marker */ })
    daemonCliPost<{ enabled: boolean }>('onboarding/harness-fanout-enabled', { project_path: projectPath })
      .then((r) => r.enabled)
      .then((on) => { if (!cancelled) setFanoutEnabled(on) })
      .catch(() => { /* default off */ })
    return () => { cancelled = true }
  }, [projectPath])

  async function toggleGenerate(): Promise<void> {
    if (generateBusy) return
    const next = !generateEnabled
    setGenerateBusy(true)
    setGenerateEnabled(next)
    setGenerateSkip(null)
    try {
      const r = await daemonCliPost<{ success?: boolean; skipped?: string }>(
        'onboarding/set-agents-md-generate-enabled',
        { project_path: projectPath, enabled: next },
      )
      if (next && r.skipped) {
        setGenerateSkip(r.skipped)
      }
    } catch (err) {
      console.error('[canonical] set_agents_md_generate_enabled failed:', err)
      setGenerateEnabled(!next)
    } finally {
      setGenerateBusy(false)
    }
  }

  async function toggleFanout(): Promise<void> {
    if (fanoutBusy) return
    const next = !fanoutEnabled
    // ENABLING is destructive-by-design — open the confirmation modal
    // instead of toggling directly. DISABLING is non-destructive, apply now.
    if (next) {
      setSkillHint(false)
      setShowFanoutModal(true)
      return
    }
    setFanoutBusy(true)
    setFanoutEnabled(false) // optimistic
    try {
      await daemonCliPost('onboarding/set-harness-fanout-enabled', { project_path: projectPath, enabled: false })
    } catch (err) {
      console.error('[canonical] set_harness_fanout_enabled failed:', err)
      setFanoutEnabled(true) // reconcile on failure
    } finally {
      setFanoutBusy(false)
    }
  }

  return (
    <div className="space-y-2">
      {/* Primary action row */}
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 flex-wrap">
            <span className="text-[11px] font-medium text-[var(--color-text-primary)]">
              K2 Canonical Agent
            </span>
            {unified ? (
              <span className="text-[8px] uppercase tracking-wider font-semibold px-1 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
                unified
              </span>
            ) : (
              <span className="text-[8px] uppercase tracking-wider font-semibold px-1 py-0.5 bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] border border-[var(--color-border)]">
                not set up
              </span>
            )}
          </div>
          <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5 leading-snug">
            Write context once — every LLM reads the same generated{' '}
            <span className="font-mono">AGENTS.md</span>.
          </p>
        </div>
        <button
          type="button"
          onClick={() => onOpen(unified ? 'manage' : 'setup')}
          className="flex-shrink-0 px-2.5 py-1 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors whitespace-nowrap no-drag cursor-pointer"
        >
          {unified ? 'Manage / Undo' : 'Set up'}
        </button>
      </div>

      {/* Generate toggle — immediately above leftover fan-out. */}
      <div className="flex items-start gap-2 pt-1.5 border-t border-[var(--color-border)]">
        <div className="min-w-0 flex-1">
          <div className="text-[10px] font-medium text-[var(--color-text-secondary)] leading-tight">
            Generate AGENTS.md
          </div>
          <p className="text-[9px] text-[var(--color-text-muted)] leading-snug mt-0.5">
            Write the cwd <span className="font-mono">AGENTS.md</span> most AI tools load
            (from persona + project + stack). Turn off to stop refreshing it; the file is left in place.
          </p>
          {generateSkip ? (
            <p className="text-[9px] text-[var(--color-status-warn-amber-soft)] leading-snug mt-0.5">
              Plant skipped: {generateSkip}
            </p>
          ) : null}
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={generateEnabled}
          aria-label="Generate cwd AGENTS.md"
          disabled={generateBusy}
          onClick={() => void toggleGenerate()}
          title={
            generateEnabled
              ? 'Generate on — refresh cwd AGENTS.md on compose'
              : 'Generate off — compose stays under .k2/AGENTS.md only'
          }
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            generateEnabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              generateEnabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {/* Fan-out toggle row — label left, switch right (stack-row alignment) */}
      <div className="flex items-start gap-2 pt-1.5 border-t border-[var(--color-border)]">
        <div className="min-w-0 flex-1">
          <div className="text-[10px] font-medium text-[var(--color-text-secondary)] leading-tight">
            Auto harness fan-out
          </div>
          <p className="text-[9px] text-[var(--color-text-muted)] leading-snug mt-0.5">
            Symlink <span className="font-mono">CLAUDE.md</span>,{' '}
            <span className="font-mono">GEMINI.md</span>, … →{' '}
            <span className="font-mono">.k2/AGENTS.md</span>. Off by default.
            {' '}
            <span className="text-[var(--color-status-warn-amber-soft)]">
              Can replace existing files
            </span>
            {' '}
            — prefer <span className="font-medium text-[var(--color-text-secondary)]">Set up</span> above
            for projects that already have harness notes.
          </p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={fanoutEnabled}
          aria-label="Programmatic harness fan-out"
          disabled={fanoutBusy}
          onClick={() => void toggleFanout()}
          title={
            fanoutEnabled
              ? 'Fan-out on — harness files symlink to .k2/AGENTS.md'
              : 'Fan-out off — enable to auto-symlink CLAUDE.md / GEMINI.md / …'
          }
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            fanoutEnabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              fanoutEnabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {skillHint ? (
        <div className="border-l-2 border-[var(--color-accent)]/60 bg-[var(--color-accent)]/5 pl-2 py-1.5 text-[9px] leading-snug text-[var(--color-text-secondary)]">
          <span className="font-medium text-[var(--color-accent)]">Canonical Agent skill ready.</span>{' '}
          Run it in this workspace&rsquo;s agent chat to merge harness files safely. Fan-out was
          <span className="font-medium"> not</span> turned on.
        </div>
      ) : null}

      {showFanoutModal ? (
        <FanoutConfirmModal
          projectPath={projectPath}
          onCancel={() => {
            setShowFanoutModal(false)
            setFanoutEnabled(false)
          }}
          onProgrammaticEnabled={() => {
            setShowFanoutModal(false)
            setFanoutEnabled(true)
          }}
          onSkillRouteTaken={() => {
            setShowFanoutModal(false)
            setFanoutEnabled(false)
            setSkillHint(true)
          }}
        />
      ) : null}
    </div>
  )
}

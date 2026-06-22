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
  'Your existing harness files (CLAUDE.md, AGENTS.md, …) are moved into .k2/migration/ before they’re replaced with symlinks to K2’s canon — nothing is deleted, and you can restore them straight from that folder if you change your mind.'

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
 * K2 Canonical Agent button — shown ALWAYS, every mode incl. custom + off
 * (PRD §9.3). Opens the canonical ceremony modal. Label gates on
 * detect_canonical_state: "Manage / Undo" when any harness is already
 * canonicalized, "Set up …" otherwise.
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
  const [fanoutEnabled, setFanoutEnabled] = useState(false)
  const [fanoutBusy, setFanoutBusy] = useState(false)
  // CHECKING the box opens the confirmation modal (replaces the bare
  // window.confirm); the modal owns the two apply routes.
  const [showFanoutModal, setShowFanoutModal] = useState(false)
  const [skillHint, setSkillHint] = useState(false)

  useEffect(() => {
    let cancelled = false
    daemonCliPost<{ enabled: boolean }>('onboarding/harness-fanout-enabled', { project_path: projectPath })
      .then((r) => r.enabled)
      .then((on) => { if (!cancelled) setFanoutEnabled(on) })
      .catch(() => { /* default off */ })
    return () => { cancelled = true }
  }, [projectPath])

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
      <div className="flex items-center justify-between">
        <div className="min-w-0">
          <span className="text-xs text-[var(--color-text-secondary)]">K2 Canonical Agent</span>
          <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">{CANONICAL_PITCH_SUBTITLE}</p>
        </div>
        <button
          onClick={() => onOpen(unified ? 'manage' : 'setup')}
          className="px-2.5 py-1 text-[11px] font-medium text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors whitespace-nowrap no-drag cursor-pointer"
        >
          {unified ? 'Manage / Undo' : 'Set up canonical'}
        </button>
      </div>
      {/* Permission checkbox lives WITH the button (PRD §4). Reads/writes the
          same `.k2/.harness-fanout-enabled` marker the Canonical Agent Flow
          settings page does, so the two stay in sync. */}
      <label className="flex items-start gap-2 cursor-pointer no-drag select-none">
        <input
          type="checkbox"
          checked={fanoutEnabled}
          disabled={fanoutBusy}
          onChange={toggleFanout}
          className="peer sr-only"
        />
        <span
          aria-hidden="true"
          className="mt-0.5 w-3 h-3 flex-shrink-0 flex items-center justify-center border transition-colors border-[var(--color-border)] bg-[var(--color-bg-elevated)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)] peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)]"
        >
          {fanoutEnabled && (
            <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M2.5 6.5 L5 9 L9.5 3.5" />
            </svg>
          )}
        </span>
        <span className="text-[9px] text-[var(--color-text-muted)] leading-snug">
          Allow programmatic harness fan-out (symlinks). When on, K2 keeps the harness files
          (<span className="font-mono">CLAUDE.md</span>, <span className="font-mono">GEMINI.md</span>, …)
          symlinked to the generated <span className="font-mono">.k2/AGENTS.md</span> automatically. Off by
          default. <span className="text-amber-300">Can overwrite existing harness content</span> — for an
          existing project, run the K2 Canonical Agent (button above) instead; it merges safely. Best for new
          projects.
        </span>
      </label>

      {skillHint ? (
        <div className="border-l-2 border-[var(--color-accent)]/60 bg-[var(--color-accent)]/5 pl-2 py-1.5 text-[9px] leading-snug text-[var(--color-text-secondary)]">
          <span className="font-medium text-[var(--color-accent)]">K2 Canonical Agent enabled.</span>{' '}
          Run it from this workspace&rsquo;s Agent chat to merge your harness files. Programmatic fan-out
          was <span className="font-medium">not</span> turned on.
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

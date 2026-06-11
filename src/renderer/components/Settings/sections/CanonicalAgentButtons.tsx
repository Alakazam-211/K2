import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { type RoleSkill, roleSkillLabel } from './canonicalAgentSeeds'
import { type HarnessProbe, anyHarnessUnified } from './canonicalState'

// The value-pitch WHY copy relocated VERBATIM from the removed consent page
// (AddWorkspaceDialog:147-164) into the canonical button subtitle + the skill
// briefing (canonical-agents PRD §7). This is where harness unification is
// actually opt-in, so the pitch belongs here.
export const CANONICAL_PITCH_SUBTITLE =
  'Tell K2 once, every AI tool listens. Each AI coding tool reads its project notes from a different file; write your context once and every tool sees the same picture.'

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
      path: `${projectPath}/.k2so/skills/${role}/SKILL.md`,
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
          <span className="font-mono">.k2so/agent/AGENT.md</span> organically — your existing
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

  useEffect(() => {
    let cancelled = false
    invoke<boolean>('k2so_harness_fanout_enabled', { projectPath })
      .then((on) => { if (!cancelled) setFanoutEnabled(on) })
      .catch(() => { /* default off */ })
    return () => { cancelled = true }
  }, [projectPath])

  async function toggleFanout(): Promise<void> {
    if (fanoutBusy) return
    const next = !fanoutEnabled
    setFanoutBusy(true)
    setFanoutEnabled(next) // optimistic
    try {
      await daemonCliPost('onboarding/set-harness-fanout-enabled', { project_path: projectPath, enabled: next })
    } catch (err) {
      console.error('[canonical] set_harness_fanout_enabled failed:', err)
      setFanoutEnabled(!next) // reconcile on failure
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
          same `.k2so/.harness-fanout-enabled` marker the Canonical Agent Flow
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
          symlinked to <span className="font-mono">.k2so/agent/AGENT.md</span> automatically. Off by default — the
          skill route (button above) is the safe, copy-based alternative.
        </span>
      </label>
    </div>
  )
}

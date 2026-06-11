import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliPost } from '@/lib/daemon-cli'
import type { SettingEntry } from '../searchManifest'
import { AgentContextDiagram } from './AgentContextDiagram'
import { useProjectsStore } from '@/stores/projects'
import { type HarnessProbe, harnessStateLabel } from './canonicalState'

// Renamed from "Agent Skills" → "Canonical Agent Flow" (canonical-agents
// PRD §11). The section is no longer a four-tier "skills shipped to tiers of
// agents" picker. Post-`agents/`-removal a workspace IS one agent: there is
// a single agent under .k2so/agent/ + a flat skills list under .k2so/skills/.
// This page is the explainer + control surface for how the workspace's
// canonical setup works: .k2so/agent/AGENT.md is THE canonical source, and
// the per-harness files (CLAUDE.md, GEMINI.md, …) are MIRRORS of it.

export const AGENT_SKILLS_MANIFEST: SettingEntry[] = [
  { id: 'agent-skills.canonical-flow', section: 'agent-skills', label: 'Canonical Agent Flow', description: 'How .k2so/agent/AGENT.md is mirrored out to the AI-harness files', keywords: ['canonical', 'agent', 'harness', 'mirror', 'fan-out', 'AGENT.md'] },
  { id: 'agent-skills.workspace-manager', section: 'agent-skills', label: 'Workspace Manager skill', description: 'Opt-in role skill — weaves Workspace Manager guidance into AGENT.md', keywords: ['manager', 'skill', 'role', 'triage', 'delegate'] },
  { id: 'agent-skills.k2-agent', section: 'agent-skills', label: 'K2 Agent skill', description: 'Opt-in role skill — weaves K2 Agent (planner) guidance into AGENT.md', keywords: ['k2', 'agent', 'planner', 'prd', 'skill', 'role'] },
  { id: 'agent-skills.k2-canonical-agent', section: 'agent-skills', label: 'K2 Canonical Agent skill', description: 'Opt-in skill — unify the workspace harness files safely (copies)', keywords: ['canonical', 'unify', 'harness', 'merge', 'mirror', 'skill'] },
  { id: 'agent-skills.harness-fanout', section: 'agent-skills', label: 'Harness fan-out (symlinks)', description: 'Per-workspace permission to programmatically symlink harness files', keywords: ['fan-out', 'symlink', 'harness', 'permission', 'checkbox'] },
]

// The three opt-in skills of this PRD (canonical-agents §2), surfaced as
// first-class entries in the flat skills list. `dir` is the .k2so/skills/<dir>
// name (matches OptInSkill::dir_name in core).
const OPT_IN_SKILLS: { dir: string; label: string; blurb: string }[] = [
  {
    dir: 'workspace-manager',
    label: 'Workspace Manager',
    blurb:
      'Role knowledge for the manager — standing orders, the k2so CLI verb surface, delegation/review. The agent weaves it into AGENT.md organically. Enable + run it from a manager workspace’s Agent section.',
  },
  {
    dir: 'k2-agent',
    label: 'K2 Agent',
    blurb:
      'Role knowledge for the planner agent — PRDs, milestones, technical plans. Woven into AGENT.md organically. Enable + run it from a K2-Agent workspace’s Agent section.',
  },
  {
    dir: 'k2-canonical-agent',
    label: 'K2 Canonical Agent',
    blurb:
      'Unifies the workspace’s AI-harness files safely: diagnose per-harness state, merge existing harness content into AGENT.md/PROJECT.md, then mirror out as backed-up, byte-reversible COPIES. Available to every workspace.',
  },
]

export function AgentSkillsSection(): React.JSX.Element {
  // Workspace-scoped surfaces (per-harness state + the fan-out permission)
  // resolve against the active project — this is a global section but the
  // canonical state + marker are per-workspace.
  const activeProject = useProjectsStore((s) =>
    s.projects.find((p) => p.id === s.activeProjectId) ?? null,
  )
  const projectPath = activeProject?.path ?? null

  const [probes, setProbes] = useState<HarnessProbe[]>([])
  const [fanoutEnabled, setFanoutEnabled] = useState(false)
  const [fanoutBusy, setFanoutBusy] = useState(false)

  const refresh = useCallback(async () => {
    if (!projectPath) {
      setProbes([])
      return
    }
    try {
      const next = await invoke<HarnessProbe[]>('k2so_detect_canonical_state', { projectPath })
      setProbes(next)
    } catch (err) {
      console.warn('[canonical-flow] detect_canonical_state failed:', err)
      setProbes([])
    }
    try {
      const on = await invoke<boolean>('k2so_harness_fanout_enabled', { projectPath })
      setFanoutEnabled(on)
    } catch (err) {
      console.warn('[canonical-flow] harness_fanout_enabled failed:', err)
    }
  }, [projectPath])

  useEffect(() => {
    refresh()
  }, [refresh])

  const toggleFanout = useCallback(async () => {
    if (!projectPath || fanoutBusy) return
    const next = !fanoutEnabled
    setFanoutBusy(true)
    // Optimistic — reflect immediately, reconcile on failure.
    setFanoutEnabled(next)
    try {
      await daemonCliPost('onboarding/set-harness-fanout-enabled', { project_path: projectPath, enabled: next })
      await refresh()
    } catch (err) {
      console.error('[canonical-flow] set_harness_fanout_enabled failed:', err)
      setFanoutEnabled(!next)
    } finally {
      setFanoutBusy(false)
    }
  }, [projectPath, fanoutEnabled, fanoutBusy, refresh])

  return (
    <div className="max-w-3xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Canonical Agent Flow</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-4">
        Each AI coding tool reads its project notes from a different file. K2 keeps{' '}
        <span className="font-mono text-[var(--color-text-secondary)]">.k2so/agent/AGENT.md</span> as
        THE canonical source — the per-harness files (
        <span className="font-mono">CLAUDE.md</span>, <span className="font-mono">GEMINI.md</span>,
        …) are <span className="text-[var(--color-text-secondary)]">mirrors</span> of it. Write your
        context once; every tool sees the same picture.
      </p>

      {/* Canonical-source diagram: AGENT.md → harness mirrors. */}
      <AgentContextDiagram />

      {/* The two opt-in routes (PRD §11). */}
      <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/30 px-3 py-2.5 mb-4 text-[11px] leading-relaxed text-[var(--color-text-secondary)]">
        <div className="font-medium text-[var(--color-text-primary)] mb-1">Two opt-in routes to canonical</div>
        <p className="mb-1.5">
          <span className="text-[var(--color-text-primary)]">Skill route (recommended).</span>{' '}
          Run the <span className="text-[var(--color-text-primary)]">K2 Canonical Agent</span> from a
          workspace’s Agent section. It writes safe <span className="text-[var(--color-text-primary)]">copies</span>{' '}
          of AGENT.md into the harness files you choose — backed up first, byte-reversible.
        </p>
        <p>
          <span className="text-[var(--color-text-primary)]">Checkbox route.</span> Enable harness
          fan-out below for ongoing <span className="text-[var(--color-text-primary)]">programmatic symlinks</span>{' '}
          pointing back at the canonical AGENT.md. Hands-off, but always-on.
        </p>
      </div>

      {/* Per-workspace permission checkbox (PRD §4 opt-in route). */}
      <div className="border border-[var(--color-border)] px-3 py-3 mb-4">
        <label className={`flex items-start gap-2.5 ${projectPath ? 'cursor-pointer' : 'opacity-50 cursor-not-allowed'}`}>
          <input
            type="checkbox"
            checked={fanoutEnabled}
            disabled={!projectPath || fanoutBusy}
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
          <div className="min-w-0">
            <div className="text-xs font-medium text-[var(--color-text-primary)]">
              Enable harness fan-out (programmatic symlinks)
            </div>
            <p className="text-[10px] text-[var(--color-text-muted)] leading-snug mt-0.5">
              When checked, K2 continuously fans the canonical{' '}
              <span className="font-mono">.k2so/agent/AGENT.md</span> out into this workspace’s harness
              files as symlinks (on boot, agent-create, agent-launch, and regen). Off by default —
              prefer the K2 Canonical Agent skill for safe copies. Scoped to{' '}
              {activeProject ? <span className="font-mono">{activeProject.name}</span> : 'the active workspace'}.
            </p>
            {!projectPath ? (
              <p className="text-[10px] text-[var(--color-text-muted)] italic mt-1">
                Select a workspace to manage its harness fan-out.
              </p>
            ) : null}
          </div>
        </label>
      </div>

      {/* The three opt-in skills as first-class entries (flat list). */}
      <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1.5">
        Opt-in skills under <span className="font-mono">.k2so/skills/</span>
      </div>
      <div className="border border-[var(--color-border)] mb-4">
        {OPT_IN_SKILLS.map((skill, i) => (
          <div
            key={skill.dir}
            className={`px-3 py-2.5 ${i < OPT_IN_SKILLS.length - 1 ? 'border-b border-[var(--color-border)]' : ''}`}
          >
            <div className="flex items-center gap-2">
              <span className="w-1 h-4 bg-[var(--color-accent)] rounded-sm flex-shrink-0" />
              <span className="text-xs font-medium text-[var(--color-text-primary)]">{skill.label}</span>
              <span className="text-[9px] font-mono text-[var(--color-text-muted)]">.k2so/skills/{skill.dir}/</span>
            </div>
            <p className="text-[10px] text-[var(--color-text-muted)] leading-snug mt-1 pl-3">{skill.blurb}</p>
          </div>
        ))}
      </div>

      {/* Live per-harness state (PRD §5.2 / §11) — copy / symlink / unmanaged. */}
      <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1.5">
        Per-harness state{activeProject ? <> · <span className="font-mono">{activeProject.name}</span></> : null}
      </div>
      <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/30">
        {!projectPath ? (
          <p className="px-3 py-2.5 text-[11px] text-[var(--color-text-muted)] italic">
            Select a workspace to see how each harness file maps back to the canonical AGENT.md.
          </p>
        ) : probes.length === 0 ? (
          <p className="px-3 py-2.5 text-[11px] text-[var(--color-text-muted)] italic">
            No harness files detected in this workspace.
          </p>
        ) : (
          probes.map((p, i) => (
            <div
              key={p.relative_path}
              className={`flex items-center justify-between gap-2 px-3 py-2 ${
                i < probes.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              }`}
            >
              <div className="min-w-0">
                <span className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                  {p.relative_path}
                </span>
                <span className="text-[10px] text-[var(--color-text-muted)] ml-2">{p.label}</span>
              </div>
              <span
                className={`text-[9px] uppercase tracking-wider px-1.5 py-0.5 border whitespace-nowrap ${
                  p.state.kind === 'unified'
                    ? 'text-emerald-300 bg-emerald-500/10 border-emerald-500/30'
                    : p.state.kind === 'unmanaged'
                      ? 'text-amber-300 bg-amber-500/10 border-amber-500/30'
                      : 'text-[var(--color-text-muted)] bg-[var(--color-bg-elevated)] border-[var(--color-border)]'
                }`}
              >
                {harnessStateLabel(p.state)}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  )
}

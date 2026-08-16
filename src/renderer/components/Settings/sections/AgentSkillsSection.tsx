import React from 'react'
import type { SettingEntry } from '../searchManifest'
import { AgentContextDiagram } from './AgentContextDiagram'

// Renamed from "Agent Skills" → "Canonical Agent Flow" (canonical-agents
// PRD §11). The section is no longer a four-tier "skills shipped to tiers of
// agents" picker. Post-`agents/`-removal a workspace IS one agent: there is
// a single agent under .k2/agent/ + a flat skills list under .k2/skills/.
// This page is the explainer + control surface for the AGENTS.md-canonical
// flow: you author .k2/agent/ROLE.md (role) + .k2/PROJECT.md (project), K2
// GENERATES the canonical .k2/AGENTS.md entrypoint from them, and the
// per-harness files (CLAUDE.md, GEMINI.md, .cursor/rules) are read-only
// symlink MIRRORS of that generated canon. AGENTS.md is the cross-tool
// standard (Pi/Hermes/Codex/+28 read it natively); CLAUDE.md is the bridge
// for Claude Code, which doesn't read AGENTS.md natively.

// Lives under Settings → General → Workspaces (help guide, not a top-level nav item).
// section stays `general` so search jumps to General; deep-link id `agent-skills` still
// redirects via the settings store.
export const AGENT_SKILLS_MANIFEST: SettingEntry[] = [
  { id: 'agent-skills.canonical-flow', section: 'general', group: 'Workspaces', label: 'Canonical Agent Flow', description: 'How the context stack (ROLE + PROJECT + optional layers) generates .k2/AGENTS.md and mirrors out to harness files', keywords: ['canonical', 'agent', 'agents.md', 'harness', 'mirror', 'fan-out', 'ROLE.md', 'context stack', 'help'] },
  { id: 'agent-skills.workspace-manager', section: 'general', group: 'Workspaces', label: 'Workspace Manager skill', description: 'Opt-in loadable role skill — manager guidance (not auto-stacked into AGENTS.md)', keywords: ['manager', 'skill', 'role', 'triage', 'delegate'] },
  { id: 'agent-skills.k2-agent', section: 'general', group: 'Workspaces', label: 'K2 Agent skill', description: 'Opt-in loadable role skill — planner guidance (not auto-stacked into AGENTS.md)', keywords: ['k2', 'agent', 'planner', 'prd', 'skill', 'role'] },
  { id: 'agent-skills.k2-canonical-agent', section: 'general', group: 'Workspaces', label: 'K2 Canonical Agent skill', description: 'Opt-in skill — unify the workspace harness files safely (merge + mirror)', keywords: ['canonical', 'unify', 'harness', 'merge', 'mirror', 'skill'] },
]

// The three opt-in skills of this PRD (canonical-agents §2), surfaced as
// first-class entries in the flat skills list. `dir` is the .k2/skills/<dir>
// name (matches OptInSkill::dir_name in core).
const OPT_IN_SKILLS: { dir: string; label: string; blurb: string }[] = [
  {
    dir: 'workspace-manager',
    label: 'Workspace Manager',
    blurb:
      'Role knowledge for the manager — standing orders, the k2 CLI verb surface, delegation/review. The agent weaves it into ROLE.md organically. Enable + run it from a manager workspace’s Agent section.',
  },
  {
    dir: 'k2-agent',
    label: 'K2 Agent',
    blurb:
      'Role knowledge for the planner agent — PRDs, milestones, technical plans. Woven into ROLE.md organically. Enable + run it from a K2-Agent workspace’s Agent section.',
  },
  {
    dir: 'k2-canonical-agent',
    label: 'K2 Canonical Agent',
    blurb:
      'Unifies the workspace’s AI-harness files safely: diagnose per-harness state, merge existing harness content into ROLE.md/PROJECT.md, regenerate the canonical .k2/AGENTS.md, then mirror it out — backed up and byte-reversible. Available to every workspace.',
  },
]

export function AgentSkillsSection(): React.JSX.Element {
  // Pure explainer for the AGENTS.md-canonical flow. The per-workspace
  // canonical state lives WITH each workspace (the Canonical Agent button +
  // checkbox + context stack editor), not here — this is a global Settings
  // section and can't switch workspaces, so showing one workspace's harness
  // state here just confused users. Removed per that feedback.
  return (
    <div className="w-full" data-settings-id="agent-skills.canonical-flow">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Canonical Agent Flow</h2>
      <div className="text-xs text-[var(--color-text-muted)] mb-4 space-y-2 leading-relaxed max-w-3xl">
        <p>
          Different AI tools look for project notes in different files. Always-on context is a{' '}
          <span className="text-[var(--color-text-secondary)]">context management stack</span>:
          pinned{' '}
          <span className="font-mono text-[var(--color-text-secondary)]">AGENT.md</span> (persona) +{' '}
          <span className="font-mono text-[var(--color-text-secondary)]">PROJECT.md</span> (project) +
          optional ordered layers (wiki index, docs, packs), composed into one entrypoint.
        </p>
        <p>
          K2 builds{' '}
          <span className="font-mono text-[var(--color-text-secondary)]">.k2/AGENTS.md</span> from
          that stack — the shared entrypoint that Pi, Hermes, Codex, and most other tools already read.
        </p>
        <p>
          Tool-specific files (
          <span className="font-mono">CLAUDE.md</span>, <span className="font-mono">GEMINI.md</span>,{' '}
          <span className="font-mono">.cursor/rules</span>
          ) are read-only symlink mirrors of that file. Claude Code only reads{' '}
          <span className="font-mono">CLAUDE.md</span>, so that mirror is its bridge.
        </p>
        <p className="text-[var(--color-text-secondary)]">
          Manage always-on context via the per-workspace stack editor (Settings → Workspaces → a
          workspace → Context) or{' '}
          <span className="font-mono">k2 agent context</span>. Write once. Every harness sees the same picture.
        </p>
      </div>

      {/* Canonical-source diagram: stack → AGENTS.md → harness mirrors. */}
      <AgentContextDiagram />

      {/* The two opt-in routes (PRD §11). */}
      <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/30 px-3 py-2.5 mb-4 text-[11px] leading-relaxed text-[var(--color-text-secondary)]">
        <div className="font-medium text-[var(--color-text-primary)] mb-1">Two opt-in routes to canonical</div>
        <p className="mb-1.5">
          <span className="text-[var(--color-text-primary)]">Skill route (recommended for existing projects).</span>{' '}
          Run the <span className="text-[var(--color-text-primary)]">K2 Canonical Agent</span> with an AI
          assistant from a workspace’s Agent section. It reads your current harness files, merges their
          content into <span className="font-mono">AGENT.md</span>/<span className="font-mono">PROJECT.md</span>{' '}
          safely, regenerates <span className="font-mono">.k2/AGENTS.md</span>, then mirrors it out — backed
          up first, byte-reversible. Nothing is overwritten.
        </p>
        <p>
          <span className="text-[var(--color-text-primary)]">Checkbox route (best for new projects).</span>{' '}
          Tick the <span className="text-[var(--color-text-primary)]">Canonical Agent</span> checkbox on the
          workspace itself for ongoing{' '}
          <span className="text-[var(--color-text-primary)]">programmatic symlinks</span> pointing back at the
          generated <span className="font-mono">.k2/AGENTS.md</span>. Hands-off, but it{' '}
          <span className="text-[var(--color-status-warn-amber-soft)]">can overwrite existing harness content</span> — for an existing
          project, prefer the skill route above.
        </p>
      </div>

      {/* The three opt-in skills as first-class entries (flat list). */}
      <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1.5">
        Opt-in skills under <span className="font-mono">.k2/skills/</span>
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
              <span className="text-[9px] font-mono text-[var(--color-text-muted)]">.k2/skills/{skill.dir}/</span>
            </div>
            <p className="text-[10px] text-[var(--color-text-muted)] leading-snug mt-1 pl-3">{skill.blurb}</p>
          </div>
        ))}
      </div>
    </div>
  )
}

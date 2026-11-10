import React from 'react'
import type { SettingEntry } from '../searchManifest'
import { AgentContextDiagram } from './AgentContextDiagram'
import { Surface } from '@/components/ui'

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

// Lives under Settings → Workspaces / Agents → Workspace settings (help
// guide, not a top-level nav item; moved from General → Workspaces in
// 0.46.5). Section `projects` + group `Workspace settings` so search opens
// that page; deep-link id `agent-skills` still redirects via the settings
// store.
export const AGENT_SKILLS_MANIFEST: SettingEntry[] = [
  { id: 'agent-skills.canonical-flow', section: 'projects', group: 'Workspace settings', label: 'Canonical Agent Flow', description: 'How the context stack (ROLE + PROJECT + optional layers) generates .k2/AGENTS.md and mirrors out to harness files', keywords: ['canonical', 'agent', 'agents.md', 'harness', 'mirror', 'fan-out', 'ROLE.md', 'context stack', 'help'] },
  { id: 'agent-skills.workspace-manager', section: 'projects', group: 'Workspace settings', label: 'Workspace Manager skill', description: 'Opt-in loadable role skill — manager guidance (not auto-stacked into AGENTS.md)', keywords: ['manager', 'skill', 'role', 'triage', 'delegate'] },
  { id: 'agent-skills.k2-agent', section: 'projects', group: 'Workspace settings', label: 'K2 Agent skill', description: 'Opt-in loadable role skill — planner guidance (not auto-stacked into AGENTS.md)', keywords: ['k2', 'agent', 'planner', 'prd', 'skill', 'role'] },
  { id: 'agent-skills.k2-canonical-agent', section: 'projects', group: 'Workspace settings', label: 'K2 Canonical Agent skill', description: 'Opt-in skill — unify the workspace harness files safely (merge + mirror)', keywords: ['canonical', 'unify', 'harness', 'merge', 'mirror', 'skill'] },
]

// The three opt-in skills of this PRD (canonical-agents §2), surfaced as
// first-class entries in the flat skills list. `dir` is the .k2/skills/<dir>
// name (matches OptInSkill::dir_name in core).
const OPT_IN_SKILLS: { dir: string; label: string; blurb: string }[] = [
  {
    dir: 'workspace-manager',
    label: 'Workspace Manager',
    blurb:
      'What a manager agent needs to know: standing orders, the k2 commands, and how to hand off and review work. The agent folds it into its role (ROLE.md). Turn it on from a manager workspace.',
  },
  {
    dir: 'k2-agent',
    label: 'K2 Agent',
    blurb:
      'What a planning agent needs to know: PRDs, milestones and technical plans. The agent folds it into its role (ROLE.md). Turn it on from a K2 Agent workspace.',
  },
  {
    dir: 'k2-canonical-agent',
    label: 'K2 Canonical Agent',
    blurb:
      'Brings a workspace’s tool files together safely: it checks each one, merges what’s in them into ROLE.md and PROJECT.md, builds .k2/AGENTS.md and links the tool files to it. Backed up first and fully undoable. Available to every workspace.',
  },
]

export function AgentSkillsSection(): React.JSX.Element {
  // Pure explainer for the AGENTS.md-canonical flow. The per-workspace
  // canonical state lives WITH each workspace (the Canonical Agent button +
  // checkbox + context stack editor), not here — this is a global Settings
  // section and can't switch workspaces, so showing one workspace's harness
  // state here just confused users. Removed per that feedback.
  return (
    <section className="w-full" data-settings-id="agent-skills.canonical-flow">
      {/* A group on the Workspace settings page, so a group heading, not a
          page title. */}
      <h3 className="type-heading text-[var(--color-text-primary)] mb-1">Canonical Agent Flow</h3>
      <div className="type-hint text-[var(--color-text-secondary)] mb-4 space-y-2 leading-relaxed max-w-[75ch]">
        <p>
          Each AI tool reads its instructions from a different file. K2 keeps one set for each
          workspace: the agent&rsquo;s role (<code>AGENT.md</code>), the project notes
          (<code>PROJECT.md</code>) and any extra layers you add, such as the wiki index.
        </p>
        <p>
          K2 joins them into one file, <code>.k2/AGENTS.md</code>, which most tools read directly.
          Tools that look elsewhere (<code>CLAUDE.md</code>, <code>GEMINI.md</code>,{' '}
          <code>.cursor/rules</code>) get a link to it that you can&rsquo;t edit by mistake, so every
          tool reads the same instructions.
        </p>
        <p>
          Change them on a workspace&rsquo;s Context tab, or with <code>k2 agent context</code>. You
          write them once.
        </p>
      </div>

      {/* Canonical-source diagram: stack → AGENTS.md → harness mirrors. */}
      <AgentContextDiagram />

      {/* The two opt-in routes (PRD §11). */}
      <Surface chamfer material="tile" className="px-4 py-3 mb-4 type-hint leading-relaxed text-[var(--color-text-secondary)]">
        <div className="type-name text-[var(--color-text-primary)] mb-1">Two ways to set it up</div>
        <p className="mb-1.5 max-w-[75ch]">
          <span className="text-[var(--color-text-primary)]">With the K2 Canonical Agent (best for an existing project).</span>{' '}
          Run it from a workspace&rsquo;s Context tab. It reads the files your tools already have, merges
          them into <code>AGENT.md</code> and <code>PROJECT.md</code>, builds <code>.k2/AGENTS.md</code> and
          links the tool files to it. It backs everything up first and can undo every change, so
          nothing is lost.
        </p>
        <p className="max-w-[75ch]">
          <span className="text-[var(--color-text-primary)]">With the Canonical Agent checkbox (best for a new project).</span>{' '}
          Tick it on the workspace and K2 keeps the tool files linked to <code>.k2/AGENTS.md</code> for
          you. It&rsquo;s hands-off, but it{' '}
          <span className="text-[var(--color-status-warn-amber-soft)]">can overwrite what&rsquo;s already in those files</span>, so
          for an existing project use the agent instead.
        </p>
      </Surface>

      {/* The three opt-in skills as first-class entries (flat list). */}
      <h3 className="type-heading text-[var(--color-text-primary)] mb-2">
        Skills you can turn on
      </h3>
      <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)] mb-4">
        {OPT_IN_SKILLS.map((skill) => (
          <div key={skill.dir} className="px-4 py-3">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="type-row text-[var(--color-text-primary)]">{skill.label}</span>
              <span className="type-hint font-mono text-[var(--color-text-secondary)]">.k2/skills/{skill.dir}/</span>
            </div>
            <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed mt-0.5 max-w-[75ch]">{skill.blurb}</p>
          </div>
        ))}
      </Surface>
    </section>
  )
}

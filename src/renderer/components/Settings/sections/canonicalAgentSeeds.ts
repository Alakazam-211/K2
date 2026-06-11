// Seed prompts for the three opt-in agent-setup skills (canonical-agents
// PRD §5 / §9). These encode the SAFETY + ORGANIC contract that the whole
// feature rests on, so they are factored out here and SNAPSHOT-TESTED
// (canonicalAgentSeeds.test.ts) — they must not silently drift.
//
//  • Role seeds (Workspace Manager / K2 Agent) tell the agent to weave the
//    role guidance into the EXISTING AGENT.md organically and NEVER inject
//    a templated block; the deterministic core backs up + writes the merge
//    atomically (PRD §3.2 / §6).
//  • Canonical seeds (setup / manage-undo) tell the K2 Canonical Agent to
//    diagnose per-harness state first, pull harness content INTO Model A,
//    produce a DRY-RUN plan, and STOP for confirmation (PRD §9.2).

/** The two role-skills that open the normal AIFileEditor on AGENT.md. */
export type RoleSkill = 'workspace-manager' | 'k2-agent'

/** Human-facing role name used in the seed text. */
export function roleSkillLabel(role: RoleSkill): string {
  return role === 'workspace-manager' ? 'Workspace Manager' : 'K2 Agent'
}

/**
 * The `--append-system-prompt` briefing for a role skill (PRD §9.1 / §3.2).
 * Verbatim safety/organic contract: read existing AGENT.md → weave in with
 * judgment → preserve context → never inject a templated block; the core
 * is the only thing that mutates the file (backup + atomic write).
 */
export function roleSeedSystemPrompt(role: RoleSkill): string {
  const label = roleSkillLabel(role)
  return [
    `Run the ${label} skill. READ the existing AGENT.md, weave the role guidance in organically with judgment, PRESERVE existing context, NEVER inject a templated block. The deterministic core backs up AGENT.md and writes your merged text atomically.`,
    ``,
    `The ${label} role knowledge lives in the skill under .k2so/skills/${role}/SKILL.md — load it, then integrate it into the user's AGENT.md (.k2so/agent/AGENT.md) without displacing the accumulated context the agent already relies on. If a section already covers the role, refine it in place rather than appending a duplicate block.`,
  ].join('\n')
}

/**
 * The final positional message that names the role skill (PRD §9.1 — the
 * "final positional seed referencing the role skill by name").
 */
export function roleSeedMessage(role: RoleSkill, projectName: string): string {
  const label = roleSkillLabel(role)
  return `Run the ${label} skill for the workspace "${projectName}". First read .k2so/agent/AGENT.md, then weave the ${label} role guidance into it organically — preserving everything the user already wrote. Show me the merged result before the core persists it.`
}

/**
 * K2 Canonical Agent — SETUP mode seed (PRD §9.2, verbatim).
 * Diagnose → ask intent → pull harness content INTO Model A → DRY-RUN plan
 * to .k2so/.canonical-setup/plan.md → STOP for confirmation.
 */
export const CANONICAL_SETUP_SEED =
  `Run the K2 Canonical Agent skill. Detect per-harness canonical state, summarize it, ask which harnesses I want and what I want to do. Pull existing harness content INTO AGENT.md/PROJECT.md first (Model A), then mirror out. Produce a DRY-RUN plan to .k2so/.canonical-setup/plan.md and STOP for confirmation before writing.`

/**
 * K2 Canonical Agent — MANAGE / UNDO mode seed (PRD §9.2, verbatim).
 * Show current per-harness state + the exact undo; on confirm run the
 * manifest-driven unwind for the chosen harnesses.
 */
export const CANONICAL_MANAGE_SEED =
  `Run the K2 Canonical Agent skill in manage mode: show the current per-harness state and the exact undo. If I confirm, run the manifest-driven unwind for the harnesses I choose.`

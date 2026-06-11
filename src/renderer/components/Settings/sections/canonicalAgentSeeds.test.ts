import { describe, it, expect } from 'vitest'
import {
  roleSeedSystemPrompt,
  roleSeedMessage,
  CANONICAL_SETUP_SEED,
  CANONICAL_MANAGE_SEED,
} from './canonicalAgentSeeds'

// These snapshots lock the safety/organic contract encoded in the seed
// prompts (canonical-agents PRD §3.2 / §9.2). If a seed changes, this test
// fails loudly so the change is a deliberate, reviewed edit — the prompts
// must not silently drift.

describe('canonical-agent seed prompts', () => {
  it('Workspace Manager role system prompt', () => {
    expect(roleSeedSystemPrompt('workspace-manager')).toMatchSnapshot()
  })

  it('K2 Agent role system prompt', () => {
    expect(roleSeedSystemPrompt('k2-agent')).toMatchSnapshot()
  })

  it('Workspace Manager role positional message', () => {
    expect(roleSeedMessage('workspace-manager', 'AcmeWorkspace')).toMatchSnapshot()
  })

  it('K2 Agent role positional message', () => {
    expect(roleSeedMessage('k2-agent', 'AcmeWorkspace')).toMatchSnapshot()
  })

  it('canonical setup seed (verbatim PRD §9.2)', () => {
    expect(CANONICAL_SETUP_SEED).toMatchSnapshot()
  })

  it('canonical manage/undo seed (verbatim PRD §9.2)', () => {
    expect(CANONICAL_MANAGE_SEED).toMatchSnapshot()
  })

  // Hard contract assertions — independent of snapshots, these enforce the
  // non-negotiable safety language so a careless snapshot-update can't strip
  // it (PRD §3.2: organic, never a templated block; core is the only mutator).
  it('role system prompts enforce the organic, never-templated contract', () => {
    for (const role of ['workspace-manager', 'k2-agent'] as const) {
      const p = roleSeedSystemPrompt(role)
      expect(p).toContain('READ the existing AGENT.md')
      expect(p).toContain('weave the role guidance in organically')
      expect(p).toContain('PRESERVE existing context')
      expect(p).toContain('NEVER inject a templated block')
      expect(p).toContain('backs up AGENT.md and writes your merged text atomically')
    }
  })

  it('canonical setup seed enforces diagnose → Model-A → dry-run → stop', () => {
    expect(CANONICAL_SETUP_SEED).toContain('Detect per-harness canonical state')
    expect(CANONICAL_SETUP_SEED).toContain('Pull existing harness content INTO AGENT.md/PROJECT.md first (Model A)')
    expect(CANONICAL_SETUP_SEED).toContain('DRY-RUN plan to .k2so/.canonical-setup/plan.md')
    expect(CANONICAL_SETUP_SEED).toContain('STOP for confirmation before writing')
  })

  it('canonical manage seed enforces show-state-then-manifest-unwind', () => {
    expect(CANONICAL_MANAGE_SEED).toContain('manage mode')
    expect(CANONICAL_MANAGE_SEED).toContain('current per-harness state and the exact undo')
    expect(CANONICAL_MANAGE_SEED).toContain('manifest-driven unwind for the harnesses I choose')
  })
})

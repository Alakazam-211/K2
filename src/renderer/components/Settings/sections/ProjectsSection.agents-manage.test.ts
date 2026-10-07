import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const src = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), 'ProjectsSection.tsx'),
  'utf8',
)

// k2 sidecar v1 (prd-k2-sidecar-cli-v1 §7.2, T3): the Agent tab row for
// "Allow hiring and managing agents" writes ONLY through the Admin route
// POST /cli/agent-access/set with toggle `agents_manage` — never the
// generic workspace/set (Member) — and patches the store optimistically.
describe('Agent-tab "Allow hiring and managing agents" toggle', () => {
  it('has a manifest id, the Agents group, and posts agent-access/set agents_manage', () => {
    expect(src).toContain("id: 'projects.agents-can-manage-agents'")
    expect(src).toContain('data-settings-id="projects.agents-can-manage-agents"')
    expect(src).toContain('<SettingsGroup title="Agents">')
    expect(src).toContain('<AgentsManageAgentsToggle project={project} />')
    expect(src).toContain('Allow hiring and managing agents')
    const start = src.indexOf('function AgentsManageAgentsToggle')
    expect(start).toBeGreaterThan(0)
    const toggle = src.slice(start, src.indexOf('// Per-workspace agents-may-manage-hosted-mail', start))
    expect(toggle).toContain("daemonCliPost(primaryScope(), 'agent-access/set'")
    expect(toggle).toContain("toggle: 'agents_manage'")
    expect(toggle).toContain('value: next')
    expect(toggle).toContain('workspace: project.path')
    expect(toggle).toContain('agentsCanManageAgents: next ? 1 : 0')
    expect(toggle).toContain('noteOptimisticProjectsMutationSuccess()')
    expect(toggle).not.toContain('fetchProjects(')
    expect(toggle).not.toContain('workspace/set')
  })
})

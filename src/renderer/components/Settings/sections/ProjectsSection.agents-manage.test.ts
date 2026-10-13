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

// K2 vault (prd-k2-vault-v1 §16.1, KV39): "Allow managing server-wide
// secrets" sits next to the row above and writes through the same
// Admin route with toggle `vault_manage`.
describe('Agent-tab "Allow managing server-wide secrets" toggle', () => {
  it('has a manifest id and posts agent-access/set vault_manage', () => {
    expect(src).toContain("id: 'projects.agents-can-manage-vault'")
    expect(src).toContain('data-settings-id="projects.agents-can-manage-vault"')
    expect(src).toContain('<AgentsManageVaultToggle project={project} />')
    expect(src.indexOf('<AgentsManageVaultToggle project={project} />')).toBeGreaterThan(
      src.indexOf('<AgentsManageAgentsToggle project={project} />'),
    )
    expect(src).toContain('Allow managing server-wide secrets')
    const start = src.indexOf('function AgentsManageVaultToggle')
    expect(start).toBeGreaterThan(0)
    const toggle = src.slice(start, src.indexOf('// K2 compute nodes — "Allow compute nodes"', start))
    expect(toggle).toContain("daemonCliPost(primaryScope(), 'agent-access/set'")
    expect(toggle).toContain("toggle: 'vault_manage'")
    expect(toggle).toContain('value: next')
    expect(toggle).toContain('workspace: project.path')
    expect(toggle).toContain('agentsCanManageVault: next ? 1 : 0')
    expect(toggle).toContain('noteOptimisticProjectsMutationSuccess()')
    expect(toggle).not.toContain('fetchProjects(')
    expect(toggle).not.toContain('workspace/set')
    // Never claim agents can't read secrets (prd §18).
    expect(toggle).not.toMatch(/agents can.t read/i)
  })
})

// K2 compute nodes (Rosson 2026-10-11): "Allow compute nodes" is the single
// per-agent compute control and lives here, with the other agent
// permissions — not in Settings → Compute. It writes through the same Admin
// route (toggle `compute`), which also grants every machine.
describe('Agent-tab "Allow compute nodes" toggle', () => {
  it('has a manifest id, sits in the Agents group, and posts agent-access/set compute', () => {
    expect(src).toContain("id: 'projects.agents-can-use-compute'")
    expect(src).toContain('data-settings-id="projects.agents-can-use-compute"')
    expect(src).toContain('<AgentsUseComputeToggle project={project} />')
    expect(src.indexOf('<AgentsUseComputeToggle project={project} />')).toBeGreaterThan(
      src.indexOf('<AgentsManageVaultToggle project={project} />'),
    )
    const start = src.indexOf('function AgentsUseComputeToggle')
    expect(start).toBeGreaterThan(0)
    const toggle = src.slice(start, src.indexOf('// Per-workspace agents-may-manage-hosted-mail', start))
    expect(toggle).toContain('setWorkspaceComputeSwitch(project.path, next)')
    expect(toggle).toContain('agentsCanUseCompute: next ? 1 : 0')
    expect(toggle).toContain('noteOptimisticProjectsMutationSuccess()')
    expect(toggle).toContain('Allow this agent to use compute nodes')
    expect(toggle).not.toContain('fetchProjects(')
    expect(toggle).not.toContain('workspace/set')
  })
})

// Node admin (Q14, 0156): "Allow this agent to operate compute nodes" sits
// right after "Allow compute nodes" and writes the same Admin route the CLI
// uses (`k2 compute operate`, toggle `compute_operate`).
describe('Agent-tab "operate compute nodes" toggle', () => {
  it('has a manifest id, follows Allow compute nodes, and posts agent-access/set compute_operate', () => {
    expect(src).toContain("id: 'projects.agents-can-operate-compute'")
    expect(src).toContain('data-settings-id="projects.agents-can-operate-compute"')
    expect(src).toContain('<AgentsOperateComputeToggle project={project} />')
    expect(src.indexOf('<AgentsOperateComputeToggle project={project} />')).toBeGreaterThan(
      src.indexOf('<AgentsUseComputeToggle project={project} />'),
    )
    const start = src.indexOf('function AgentsOperateComputeToggle')
    expect(start).toBeGreaterThan(0)
    const toggle = src.slice(start, src.indexOf('// Per-workspace agents-may-manage-hosted-mail', start))
    expect(toggle).toContain('setWorkspaceComputeOperate(project.path, next)')
    expect(toggle).toContain('agentsCanOperateCompute: next ? 1 : 0')
    expect(toggle).toContain('noteOptimisticProjectsMutationSuccess()')
    expect(toggle).toContain('fetchViewerRole()')
    expect(toggle).not.toContain('fetchProjects(')
    expect(toggle).not.toContain('workspace/set')
  })
})

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const src = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), 'ProjectsSection.tsx'),
  'utf8',
)

describe('Agent-tab Skin Access toggle', () => {
  it('has manifest id, dedicated POST, optimistic patch, never GET /cli/users', () => {
    expect(src).toContain("id: 'projects.agents-can-manage-skin'")
    expect(src).toContain("data-settings-id=\"projects.agents-can-manage-skin\"")
    expect(src).toContain('<SettingsGroup title="Skin Access">')
    expect(src).toContain('Allow this agent to manage Skin Access.')
    const toggleStart = src.indexOf('function AgentsManageSkinToggle')
    expect(toggleStart).toBeGreaterThan(0)
    const toggle = src.slice(toggleStart, src.indexOf('function AgentsCreateConnectionsToggle', toggleStart))
    expect(toggle).toContain("daemonCliPost('agents-manage-skin'")
    expect(toggle).toContain('enable: next ? 1 : 0')
    expect(toggle).toContain('noteOptimisticProjectsMutationSuccess()')
    expect(toggle).not.toContain('fetchProjects(')
    expect(toggle).not.toContain("daemonCliGet('users'")
    expect(toggle).not.toContain("daemonCliGet('skin/users'")
    expect(toggle).not.toContain('/cli/users')
    expect(toggle).not.toContain('workspace/set')
  })
})

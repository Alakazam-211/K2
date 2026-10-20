import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const here = dirname(fileURLToPath(import.meta.url))
const src = readFileSync(join(here, 'ProjectsSection.tsx'), 'utf8')
const card = readFileSync(join(here, 'AgentAppsAccess.tsx'), 'utf8')

describe('Agent-tab Apps card (prd-agent-app-permissions-v1 slice H)', () => {
  it('sits in the Apps group, keeps the search id, writes one route, never refetches projects', () => {
    expect(src).toContain("id: 'projects.agents-can-manage-skin'")
    expect(src).toContain('<SettingsGroup title="Apps">')
    expect(src).toContain('<AgentAppsAccess project={project} />')
    expect(src).not.toContain('function AgentsManageSkinToggle')
    expect(src).not.toContain('<SettingsGroup title="Skin Access">')
    expect(card).toContain('settingsId="projects.agents-can-manage-skin"')
    expect(card).toContain('noteOptimisticProjectsMutationSuccess()')
    expect(card).not.toContain('fetchProjects(')
    expect(card).not.toContain("'agents-manage-skin'")
    expect(card).not.toContain('/cli/users')
    expect(card).not.toContain('workspace/set')
  })
})

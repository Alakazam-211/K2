// @vitest-environment jsdom

// The Agent tab's "Can ask for permission cards" (Beta, PRD §2.2): where it
// sits in Settings → Workspaces → <agent> → Agent, its search entry beside
// pg-b's Postgres row, and the "Asked for this" chips on the switch rows.
// The row's behaviour is tested in PermissionCardsSection.test.tsx.

import { describe, it, expect, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))

import { PROJECTS_MANIFEST } from './ProjectsSection'

const src = readFileSync(join(__dirname, 'ProjectsSection.tsx'), 'utf8')

describe('Agent tab wiring', () => {
  it('the row sits last in the Agents group, under the other agent switches', () => {
    const gardens = src.indexOf('<AgentsManageGardensToggle project={project} />')
    const row = src.indexOf('<AgentPermissionCardsRow project={project} />')
    expect(gardens).toBeGreaterThan(-1)
    expect(row).toBeGreaterThan(gardens)
    const groupEnd = src.indexOf('</SettingsGroup>', gardens)
    expect(row).toBeLessThan(groupEnd)
  })

  it('the Agent and Connections tabs are wrapped in PermissionAsksProvider', () => {
    expect(src).toContain("{settingsTab === 'agent' && (\n          <PermissionAsksProvider project={project}>")
    expect(src).toContain("{settingsTab === 'connections' && (\n          <PermissionAsksProvider project={project}>")
    expect(src.match(/<\/PermissionAsksProvider>/g)?.length).toBe(2)
  })

  it('search finds it, right after the Postgres row', () => {
    const ids = PROJECTS_MANIFEST.map((e) => e.id)
    const pg = ids.indexOf('projects.agents-can-upgrade-postgres')
    expect(pg).toBeGreaterThan(-1)
    expect(ids[pg + 1]).toBe('projects.agents-can-request-permissions')
    const entry = PROJECTS_MANIFEST[pg + 1]
    expect(entry.section).toBe('projects')
    expect(entry.label).toBe('Can ask for permission cards')
    expect(entry.keywords).toContain('permission_cards')
  })

  it('the per-agent value never comes from Project JSON (C8)', () => {
    expect(src).not.toMatch(/agentsCanRequestPermissions/)
  })
})

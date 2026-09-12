import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const src = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), 'ProjectsSection.tsx'),
  'utf8',
)

describe('Hosted mail manage toggle', () => {
  it('has manifest id, dedicated POST, optimistic patch, after DNS, never fetchProjects', () => {
    expect(src).toContain("id: 'projects.mail-manage-enabled'")
    expect(src).toContain('data-settings-id="projects.mail-manage-enabled"')
    expect(src).toContain('<SettingsGroup title="Hosted mail">')
    expect(src).toContain('Allow agents to manage hosted mail on this host')
    const dnsGroup = src.indexOf('<SettingsGroup title="DNS">')
    const mailGroup = src.indexOf('<SettingsGroup title="Hosted mail">')
    const dbGroup = src.indexOf('<SettingsGroup title="Database">')
    expect(dnsGroup).toBeGreaterThan(0)
    expect(mailGroup).toBeGreaterThan(dnsGroup)
    expect(dbGroup).toBeGreaterThan(mailGroup)
    const toggleStart = src.indexOf('function MailManageToggle')
    expect(toggleStart).toBeGreaterThan(0)
    const toggle = src.slice(toggleStart, src.indexOf('function DbAgentCreateToggle', toggleStart))
    expect(toggle).toContain("daemonCliPost('mail-manage'")
    expect(toggle).toContain('enable: next ? 1 : 0')
    expect(toggle).toContain('noteOptimisticProjectsMutationSuccess()')
    expect(toggle).not.toContain('await fetchProjects')
    expect(toggle).not.toContain("daemonCliGet('dns-manage'")
    expect(toggle).not.toContain("daemonCliGet('mail-manage'")
    expect(toggle).not.toContain('Currently allowed anyway')
  })
})

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
    expect(src).toContain('<SettingsGroup title="Hosted mail" card>')
    expect(src).toContain('Let this agent manage hosted mail')
    const dnsGroup = src.indexOf('<SettingsGroup title="DNS" card>')
    const mailGroup = src.indexOf('<SettingsGroup title="Hosted mail" card>')
    const dbGroup = src.indexOf('<SettingsGroup title="Database" card>')
    expect(dbGroup).toBeGreaterThan(0)
    expect(mailGroup).toBeGreaterThan(dbGroup)
    expect(dnsGroup).toBeGreaterThan(mailGroup)
    const toggleStart = src.indexOf('function MailManageToggle')
    expect(toggleStart).toBeGreaterThan(0)
    const toggle = src.slice(toggleStart, src.indexOf('function DbAgentCreateToggle', toggleStart))
    expect(toggle).toContain("daemonCliPost(primaryScope(), 'mail-manage'")
    expect(toggle).toContain('enable: next ? 1 : 0')
    expect(toggle).toContain('noteOptimisticProjectsMutationSuccess()')
    expect(toggle).not.toContain('await fetchProjects')
    expect(toggle).not.toContain("daemonCliGet(primaryScope(), 'dns-manage'")
    expect(toggle).not.toContain("daemonCliGet(primaryScope(), 'mail-manage'")
    expect(toggle).not.toContain('Currently allowed anyway')
  })

  // 0.46.18: the workspace's address cap shows under the switch, in the
  // daemon's own words ("Addresses: unlimited (this agent manages email
  // hosting)"), read again when the switch flips; never fetchProjects.
  it('shows the address cap line under the switch, from the daemon', () => {
    const group = src.slice(
      src.indexOf('<SettingsGroup title="Hosted mail" card>'),
      src.indexOf('</SettingsGroup>', src.indexOf('<SettingsGroup title="Hosted mail" card>')),
    )
    expect(group).toContain('<MailManageToggle project={project} />')
    expect(group).toContain('<MailAddressCapLine project={project} />')
    const start = src.indexOf('function MailAddressCapLine')
    expect(start).toBeGreaterThan(0)
    const line = src.slice(start, src.indexOf('function MailAgentSendControl', start))
    expect(line).toContain("primaryScope(), 'mail/config', { workspace: project.path }")
    expect(line).toContain('d.workspace?.addressCap?.text')
    expect(line).toContain('d.supported === true')
    expect(line).toContain('[project.path, manages]')
    expect(line).not.toContain('fetchProjects')
  })
})

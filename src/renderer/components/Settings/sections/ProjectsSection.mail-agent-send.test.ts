import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const src = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), 'ProjectsSection.tsx'),
  'utf8',
)

// The `gated` hint on `k2 mail send` / `reply` names this control
// ("Settings → Workspaces / Agents → <workspace> → Email sending"), so it
// must exist, read and write the daemon's value, and not be hosted-only.
describe('Email sending control (per-workspace agent-send)', () => {
  it('has manifest id, the group the hint names, daemon read + write, never fetchProjects', () => {
    expect(src).toContain("id: 'projects.mail-agent-send'")
    expect(src).toContain('data-settings-id="projects.mail-agent-send"')
    expect(src).toContain('<SettingsGroup title="Email sending">')
    const dnsGroup = src.indexOf('<SettingsGroup title="DNS">')
    const sendGroup = src.indexOf('<SettingsGroup title="Email sending">')
    const hostedGroup = src.indexOf('<SettingsGroup title="Hosted mail">')
    expect(sendGroup).toBeGreaterThan(dnsGroup)
    expect(hostedGroup).toBeGreaterThan(sendGroup)

    const start = src.indexOf('function MailAgentSendControl')
    expect(start).toBeGreaterThan(0)
    const body = src.slice(
      start,
      src.indexOf('// Per-workspace agents-may-manage-hosted-mail', start),
    )
    expect(body.length).toBeGreaterThan(100)
    expect(body).toContain("daemonCliGet<")
    expect(body).toContain("'mail/config', { workspace: project.path }")
    expect(body).toContain("daemonCliPost(primaryScope(), 'mail/config/set'")
    expect(body).toContain('agentSend: next')
    for (const mode of ["value: 'off'", "value: 'approval'", "value: 'on'"]) {
      expect(body).toContain(mode)
    }
    // Not gated on the hosted-mail `supported` flag: works on macOS.
    expect(body).not.toContain('supported')
    expect(body).not.toContain('fetchProjects')
  })
})

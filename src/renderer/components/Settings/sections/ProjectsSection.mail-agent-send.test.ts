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
  it('says plainly what sending on allows, beside the setting', () => {
    const at = src.indexOf('function MailAgentSendControl')
    const body = src.slice(at, src.indexOf('ariaLabel="Agent email sending"', at))
    expect(body).toContain('mail-agent-send-on-warning')
    expect(body).toContain('a stranger&apos;s email can trick this agent into sending your mail and files to anyone')
    expect(body).toContain('Choose Approval for any agent that reads mail from outside.')
  })

  it('has manifest id, the group the hint names, daemon read + write, never fetchProjects', () => {
    expect(src).toContain("id: 'projects.mail-agent-send'")
    expect(src).toContain('data-settings-id="projects.mail-agent-send"')
    expect(src).toContain('<SettingsGroup title="Email sending" card>')
    const dnsGroup = src.indexOf('<SettingsGroup title="DNS" card>')
    const sendGroup = src.indexOf('<SettingsGroup title="Email sending" card>')
    const hostedGroup = src.indexOf('<SettingsGroup title="Hosted mail" card>')
    expect(sendGroup).toBeGreaterThan(0)
    expect(hostedGroup).toBeGreaterThan(sendGroup)
    expect(dnsGroup).toBeGreaterThan(hostedGroup)

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

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { SECTION_LABELS } from './searchManifest'

const src = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), 'Settings.tsx'),
  'utf8',
)

describe('Settings nav Apps', () => {
  it('sidebar label is Apps, not Skin Access; route id stays skin-access', () => {
    expect(src).toContain("{ id: 'skin-access', label: 'Apps', beta: true }")
    expect(src).not.toContain("{ id: 'skin-access', label: 'Skin Access' }")
    expect(src).toContain("id: 'k2-access', label: 'Admin Access'")
    expect(src).toContain("activeSection === 'skin-access'")
    expect(src).not.toContain("id: 'k2-access', label: 'Admin Access', beta")
  })

  it('K2 Server order is Admin Access, User Access, User Templates, Custom Domains, Tunnel, Connected Servers, API Keys, K2 Companion', () => {
    const serverStart = src.indexOf("title: 'K2 Server'")
    const serverEnd = src.indexOf("title: 'Editors / Fonts'")
    expect(serverStart).toBeGreaterThan(0)
    expect(serverEnd).toBeGreaterThan(serverStart)
    const server = src.slice(serverStart, serverEnd)
    expect(server).toContain(
      [
        "{ id: 'k2-access', label: 'Admin Access' },",
        "{ id: 'people', label: 'User Access', beta: true },",
        "{ id: 'user-templates', label: 'User Templates', beta: true },",
        "{ id: 'domains', label: 'Custom Domains' },",
        "{ id: 'k2-connect', label: 'Tunnel', hide: hideTunnel },",
        "{ id: 'connections', label: 'Connected Servers' },",
        "{ id: 'api-tokens', label: 'API Keys' },",
        "{ id: 'companion', label: 'K2 Companion' },",
      ].join('\n        '),
    )
    expect(server).not.toContain("id: 'skin-access'")
    expect(server).not.toContain("{ id: 'skin-access', label: 'People' }")
    expect(src).toContain("id: 'k2-access'")
    expect(src).toContain("id: 'people'")
    expect(src).toContain("id: 'skin-access'")
    expect(src).toContain("activeSection === 'people'")
    expect(src).toContain("activeSection === 'user-templates'")
    expect(src).toContain('<PeopleSection />')
    expect(src).toContain('<UserTemplatesSection />')
    expect(src).not.toContain("{ id: 'k2-access', label: 'People' }")
    expect(SECTION_LABELS.domains).toBe('Custom Domains')
    expect(SECTION_LABELS['user-templates']).toBe('User Templates')
    expect(SECTION_LABELS.people).toBe('User Access')
    expect(SECTION_LABELS['k2-access']).toBe('Admin Access')
    expect(SECTION_LABELS['skin-access']).toBe('Apps')
  })

  it('User Access, User Templates, and Apps rows carry a right-side BETA tag', () => {
    expect(src).toContain('justify-between')
    expect(src).toContain('ml-auto')
    expect(src).toContain('>BETA<')
    expect(src).toContain('beta={it.beta}')
    const button = src.slice(src.indexOf('function SettingsNavButton'))
    expect(button).toContain('justify-between')
    expect(button).toContain('ml-auto')
    expect(button).toContain('BETA')
  })
})

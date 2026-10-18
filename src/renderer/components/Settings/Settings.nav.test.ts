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

  it('K2 Server is two groups (0.45.7): Access (Admin Access, User Access, User Templates, API Keys, Vault) and Network (Tunnel, Custom Domains, Connected Servers, K2 Companion)', () => {
    expect(src).not.toContain("title: 'K2 Server'")
    const accessStart = src.indexOf("title: 'Access'")
    const networkStart = src.indexOf("title: 'Network'")
    const editors = src.indexOf("title: 'Editors / Fonts'")
    expect(accessStart).toBeGreaterThan(src.indexOf("title: 'Language Models'"))
    expect(networkStart).toBeGreaterThan(accessStart)
    expect(editors).toBeGreaterThan(networkStart)
    // Gardens sits between Access and Network (Rosson 2026-10-15).
    const access = src.slice(accessStart, src.indexOf("title: 'Gardens'"))
    expect(src.indexOf("title: 'Gardens'")).toBeGreaterThan(accessStart)
    const network = src.slice(networkStart, editors)
    expect(access).toContain(
      [
        "{ id: 'k2-access', label: 'Admin Access' },",
        "{ id: 'people', label: 'User Access', beta: true },",
        "{ id: 'user-templates', label: 'User Templates', beta: true },",
        "{ id: 'api-tokens', label: 'API Keys' },",
        "{ id: 'vault', label: 'Vault' },",
      ].join('\n        '),
    )
    expect(network).toContain(
      [
        "{ id: 'k2-connect', label: 'Tunnel', hide: hideTunnel },",
        "{ id: 'domains', label: 'Custom Domains' },",
        "{ id: 'connections', label: 'Connected Servers' },",
        "{ id: 'companion', label: 'K2 Companion' },",
        '// homes-sync-v1 (0.46.1): your Homes between this computer and your',
        '// phone; the devices syncing from this server, with Remove.',
        "{ id: 'homes-sync', label: 'Homes Sync' },",
      ].join('\n        '),
    )
    // External Data is in Network only where the Gardens group is hidden.
    expect(network).toContain("...(zenGardens ? [] : [{ id: 'data-sources' as const, label: 'External Data' }]),")
    expect(access).not.toContain("id: 'data-sources'")
    expect(access + network).not.toContain("id: 'local-grokbots'")
    expect(access + network).not.toContain("id: 'skin-access'")
    expect(src).not.toContain("{ id: 'skin-access', label: 'People' }")
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
    // prd-k2-vault-v1 KV39: Settings → Access → Vault.
    expect(SECTION_LABELS.vault).toBe('Vault')
    expect(src).toContain("activeSection === 'vault'")
    expect(src).toContain('<VaultSection />')
    expect(src).toContain('...VAULT_MANIFEST,')
    // contract-sources-v1 §8: Settings → Gardens → External Data.
    expect(SECTION_LABELS['data-sources']).toBe('External Data')
    expect(src).toContain("activeSection === 'data-sources'")
    expect(src).toContain('<DataSourcesSection />')
    expect(src).toContain('...DATA_SOURCES_MANIFEST,')
    // Local Grok Bots is 3rd Party Bots → Grok Bot, from a provider table.
    expect(src).toContain("{ id: 'local-grokbots', label: 'Grok Bot', capability: 'grokbots-v1' },")
    expect(src).toContain("title: '3rd Party Bots'")
    expect(SECTION_LABELS['local-grokbots']).toBe('Grok Bot')
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

describe('Settings nav Gardens group (0.45.7, Rosson: Gardens get their own section)', () => {
  it('Gardens is a group like Access and Logs: Gardens, Local Data, External Data, beneath Access', () => {
    const start = src.indexOf("title: 'Gardens'")
    const access = src.indexOf("title: 'Access'")
    const network = src.indexOf("title: 'Network'")
    expect(start).toBeGreaterThan(access)
    expect(network).toBeGreaterThan(start)
    const gardens = src.slice(start, network)
    expect(gardens).toContain(
      [
        "{ id: 'zen-gardens', label: 'Gardens' },",
        "{ id: 'zen-widget-data', label: 'Local Data' },",
        "{ id: 'data-sources', label: 'External Data' },",
      ].join('\n              '),
    )
    // No longer a top-level page.
    expect(src).not.toContain("{ kind: 'item', id: 'zen-gardens'")
    // Both pages route, search and fall back like the rest.
    expect(src).toContain("activeSection === 'zen-gardens'")
    expect(src).toContain("activeSection === 'zen-widget-data'")
    expect(src).toContain('<ZenGardensSection />')
    expect(src).toContain('<ZenWidgetDataSection />')
    expect(src).toContain('...(zenAvailable() ? ZEN_GARDENS_MANIFEST : []),')
    expect(src).toContain('...(zenAvailable() ? ZEN_WIDGET_DATA_MANIFEST : []),')
    expect(src).toContain("(activeSection === 'zen-gardens' || activeSection === 'zen-widget-data')")
    expect(SECTION_LABELS['zen-gardens']).toBe('Gardens')
    expect(SECTION_LABELS['zen-widget-data']).toBe('Local Data')
  })
})

describe('Settings nav Language Models group (0.46, Rosson: Subs / Tokens · Providers / Models · Usage)', () => {
  it('Language Models is a group: Subs / Tokens, Providers / Models, Usage, before Access; Gardens follows Access', () => {
    const start = src.indexOf("title: 'Language Models'")
    const gardens = src.indexOf("title: 'Gardens'")
    const server = src.indexOf("title: 'Access'")
    expect(server).toBeGreaterThan(start)
    expect(gardens).toBeGreaterThan(server)
    expect(src.slice(start, server)).toContain(
      [
        "{ id: 'llm-tokens', label: 'Subs / Tokens' },",
        "{ id: 'agents', label: 'Providers / Models' },",
        "{ id: 'token-usage', label: 'Usage' },",
      ].join('\n        '),
    )
    // No longer top-level pages; the old ids are the leaves, so deep links
    // (openSettings('agents'), openSettings('token-usage')) land in the group.
    expect(src).not.toContain("{ kind: 'item', id: 'agents'")
    expect(src).not.toContain("{ kind: 'item', id: 'token-usage'")
    expect(src).toContain("activeSection === 'agents'")
    expect(src).toContain("activeSection === 'llm-tokens'")
    expect(src).toContain("activeSection === 'token-usage'")
    expect(src).toContain('<AgentsSection />')
    expect(src).toContain('<LlmTokensSection />')
    expect(src).toContain('<TokenUsageSection />')
    expect(src).toContain('...AGENTS_MANIFEST,')
    expect(src).toContain('...LLM_TOKENS_MANIFEST,')
    expect(src).toContain('...TOKEN_USAGE_MANIFEST,')
    expect(src).not.toContain("title: 'LLMs'")
    expect(SECTION_LABELS.agents).toBe('Providers / Models')
    expect(SECTION_LABELS['llm-tokens']).toBe('Subs / Tokens')
    expect(SECTION_LABELS['token-usage']).toBe('Usage')
    expect(SECTION_LABELS['editors-agents']).toBe('Providers / Models')
  })
})

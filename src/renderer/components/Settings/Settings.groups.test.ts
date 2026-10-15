// @vitest-environment jsdom
// Settings sidebar groups (0.45.7): the nav blocks and where old ids land.
import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { settingsNav, THIRD_PARTY_BOTS } from './Settings'
import { useSettingsStore, type SettingsSection } from '@/stores/settings'
import { primaryScope } from '@/kessel/server-scope'
import { SECTION_LABELS } from './searchManifest'
import { COMING_SOON_BOTS, COMING_SOON_BOTS_MANIFEST } from './sections/ComingSoonBotSection'

type Nav = ReturnType<typeof settingsNav>
const titles = (nav: Nav): string[] => nav.flatMap((b) => (b.kind === 'group' ? [b.title] : []))
const group = (nav: Nav, title: string) => {
  const g = nav.find((b) => b.kind === 'group' && b.title === title)
  if (!g || g.kind !== 'group') throw new Error(`no group ${title}`)
  return g
}
const leafIds = (nav: Nav): Array<SettingsSection | undefined> =>
  nav.flatMap((b) => (b.kind === 'item' ? [b.id] : b.items.map((i) => i.id)))

describe('Settings → LLMs group', () => {
  it('is one group with LLMs, Tokens, Token Usage, whether or not Gardens show', () => {
    for (const zenGardens of [true, false]) {
      const nav = settingsNav({ zenGardens })
      const group = nav.find((b) => b.kind === 'group' && b.title === 'LLMs')
      expect(group).toEqual({
        kind: 'group',
        title: 'LLMs',
        items: [
          { id: 'agents', label: 'LLMs' },
          { id: 'llm-tokens', label: 'Tokens' },
          { id: 'token-usage', label: 'Token Usage' },
        ],
      })
      const items = nav.flatMap((b) => (b.kind === 'item' ? [b.id] : []))
      expect(items).not.toContain('agents')
      expect(items).not.toContain('token-usage')
      const titles = nav.flatMap((b) => (b.kind === 'group' ? [b.title] : []))
      expect(titles[0]).toBe('LLMs')
    }
  })

  it('old ids land on the right leaf', () => {
    const s = useSettingsStore.getState()
    s.openSettings('agents')
    expect(useSettingsStore.getState().activeSection).toBe('agents')
    s.openSettings('token-usage')
    expect(useSettingsStore.getState().activeSection).toBe('token-usage')
    s.openSettings('editors-agents')
    expect(useSettingsStore.getState().activeSection).toBe('agents')
    s.setSection('llm-tokens')
    expect(useSettingsStore.getState().activeSection).toBe('llm-tokens')
    s.closeSettings()
  })
})

describe('Settings: K2 Server becomes Access and Network (0.45.7)', () => {
  it('group order: LLMs, Access, Gardens, Network, Editors / Fonts, Logs, Cores, 3rd Party Bots', () => {
    expect(titles(settingsNav({ zenGardens: true }))).toEqual([
      'LLMs', 'Access', 'Gardens', 'Network', 'Editors / Fonts', 'Logs', 'Cores', '3rd Party Bots',
    ])
    expect(titles(settingsNav({ zenGardens: false }))).toEqual([
      'LLMs', 'Access', 'Network', 'Editors / Fonts', 'Logs', 'Cores', '3rd Party Bots',
    ])
  })

  it('Access and Network hold the old K2 Server pages; every old id is still a leaf exactly once', () => {
    for (const zenGardens of [true, false]) {
      const nav = settingsNav({ zenGardens })
      expect(group(nav, 'Access').items.map((i) => i.id)).toEqual(['k2-access', 'people', 'user-templates', 'api-tokens', 'vault'])
      expect(group(nav, 'Network').items.map((i) => i.id)).toEqual(
        zenGardens
          ? ['k2-connect', 'domains', 'connections', 'companion']
          : ['k2-connect', 'domains', 'connections', 'companion', 'data-sources'],
      )
      const ids = leafIds(nav)
      for (const id of [
        'k2-access', 'people', 'user-templates', 'domains', 'k2-connect', 'connections',
        'api-tokens', 'vault', 'data-sources', 'local-grokbots', 'companion', 'access-audit',
      ] as const) {
        expect(ids.filter((x) => x === id).length, id).toBe(1)
      }
    }
    // Access Audit stays in Logs.
    expect(group(settingsNav({ zenGardens: true }), 'Logs').items.map((i) => i.id)).toContain('access-audit')
  })

  it('3rd Party Bots: Grok Bot (hidden without grokbots-v1), then Hermes, Instinct, Muse, Dots tagged Soon; the group always shows', () => {
    expect(THIRD_PARTY_BOTS).toEqual([{ id: 'local-grokbots', label: 'Grok Bot', capability: 'grokbots-v1' }])
    const soon = [
      { id: 'bots-hermes', label: 'Hermes', comingSoon: true },
      { id: 'bots-instinct', label: 'Instinct', comingSoon: true },
      { id: 'bots-muse', label: 'Muse', comingSoon: true },
      { id: 'bots-dots', label: 'Dots', comingSoon: true },
    ]
    // The window's server decides: stub what it reports.
    let supported = new Set<string>()
    const spy = vi
      .spyOn(primaryScope(), 'serverSupports')
      .mockImplementation((f: unknown) => supported.has(String(f)))
    expect(group(settingsNav({ zenGardens: true }), '3rd Party Bots').items).toEqual([
      { id: 'local-grokbots', label: 'Grok Bot', hide: true },
      ...soon,
    ])
    supported = new Set(['grokbots-v1'])
    expect(group(settingsNav({ zenGardens: true }), '3rd Party Bots').items).toEqual([
      { id: 'local-grokbots', label: 'Grok Bot', hide: false },
      ...soon,
    ])
    spy.mockRestore()
    expect(COMING_SOON_BOTS.map((b) => b.cliPrefix)).toEqual(['hm', 'in', 'ms', 'dt'])
    for (const b of COMING_SOON_BOTS) {
      expect(SECTION_LABELS[b.id]).toBe(b.name)
      expect(COMING_SOON_BOTS_MANIFEST.find((e) => e.section === b.id)?.label).toBe(b.name)
    }
  })

  it('Sidecars is now Cores (label only; the Apps, Database and Email Hosting ids stay)', () => {
    const nav = settingsNav({ zenGardens: true })
    expect(titles(nav)).not.toContain('Sidecars')
    expect(group(nav, 'Cores').items.map((i) => i.id)).toEqual(['email-hosting', undefined, 'data', 'skin-access', 'compute'])
    const chat = group(nav, 'Cores').items[1]
    expect(chat.label).toBe('Chat Hosting')
    expect(chat.soon).toBe(true)
    // Its tag matches the 3rd Party Bots SOON tag: same classes, right-justified.
    const settingsSrc = readFileSync(join(__dirname, 'Settings.tsx'), 'utf8')
    const tag =
      '<span className="ml-auto text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-text-muted)]/15 text-[var(--color-text-muted)]">SOON</span>'
    expect(settingsSrc.split(tag).length - 1).toBe(2)
    expect(settingsSrc).not.toContain('>\n                          soon\n')
  })

  it('Key Bindings is under Editors / Fonts and Compute is under Cores; neither is top-level', () => {
    const nav = settingsNav({ zenGardens: true })
    expect(group(nav, 'Editors / Fonts').items.map((i) => i.id)).toEqual(['terminal', 'code-editor', 'keybindings', 'editors'])
    const top = nav.flatMap((b) => (b.kind === 'item' ? [b.id] : []))
    expect(top).not.toContain('keybindings')
    expect(top).not.toContain('compute')
  })

  it('old ids still open the same pages', () => {
    const s = useSettingsStore.getState()
    const ids = [
      'k2-access', 'people', 'user-templates', 'api-tokens', 'vault', 'k2-connect',
      'domains', 'connections', 'companion', 'data-sources', 'local-grokbots',
    ] as const
    for (const id of ids) {
      s.openSettings(id)
      expect(useSettingsStore.getState().activeSection).toBe(id)
    }
    s.closeSettings()
  })

  it('each Access page links to Logs → Access Audit', () => {
    const dir = join(process.cwd(), 'src/renderer/components/Settings')
    for (const f of ['PeopleSection', 'UserTemplatesSection', 'ApiTokensSection', 'VaultSection']) {
      const src = readFileSync(join(dir, `sections/${f}.tsx`), 'utf8')
      expect(src, f).toContain('<AccessAuditLink className="mt-1" />')
    }
    // Admin Access shares its shell with Tunnel and Connected Servers: only its tab shows the link.
    const shell = readFileSync(join(dir, 'sections/K2ConnectSettingsShell.tsx'), 'utf8')
    expect(shell).toContain("{tab === 'people' && <AccessAuditLink />}")
  })
})

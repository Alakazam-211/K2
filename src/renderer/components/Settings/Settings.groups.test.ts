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
import { SECTION_LABELS, searchManifest } from './searchManifest'
import { AGENTS_MANIFEST } from './sections/AgentsSection'
import { LLM_TOKENS_MANIFEST } from './sections/LlmTokensSection'
import { TOKEN_USAGE_MANIFEST } from './sections/TokenUsageSection'
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

describe('Settings → Language Models group (was LLMs)', () => {
  it('is one group with Subs / Tokens, Providers / Models, Usage (in that order), whether or not Gardens show', () => {
    for (const zenGardens of [true, false]) {
      const nav = settingsNav({ zenGardens })
      const group = nav.find((b) => b.kind === 'group' && b.title === 'Language Models')
      expect(group).toEqual({
        kind: 'group',
        title: 'Language Models',
        items: [
          { id: 'llm-tokens', label: 'Subs / Tokens' },
          { id: 'agents', label: 'Providers / Models' },
          { id: 'token-usage', label: 'Usage' },
        ],
      })
      const items = nav.flatMap((b) => (b.kind === 'item' ? [b.id] : []))
      expect(items).not.toContain('agents')
      expect(items).not.toContain('token-usage')
      const titles = nav.flatMap((b) => (b.kind === 'group' ? [b.title] : []))
      expect(titles[0]).toBe('Language Models')
      expect(titles).not.toContain('LLMs')
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

describe('Settings search: Language Models pages (was LLMs)', () => {
  const llm = [...AGENTS_MANIFEST, ...LLM_TOKENS_MANIFEST, ...TOKEN_USAGE_MANIFEST]
  const ids = (q: string): string[] => searchManifest(llm, q).map((e) => e.id)

  it('labels the pages Models, Subs / Tokens, Usage', () => {
    expect(LLM_TOKENS_MANIFEST.find((e) => e.id === 'agents.accounts')!.label).toBe('Subs / Tokens')
    expect(TOKEN_USAGE_MANIFEST.find((e) => e.id === 'token-usage.machine')!.label).toBe('Usage')
    expect(SECTION_LABELS.agents).toBe('Providers / Models')
  })

  it('old and new words both find them', () => {
    expect(ids('Subscriptions')[0]).toBe('agents.accounts')
    expect(ids('tokens')).toContain('agents.accounts')
    expect(ids('LLM')).toEqual(expect.arrayContaining(['agents.accounts', 'token-usage.machine', 'agents.default-agent']))
    expect(ids('language models')).toEqual(expect.arrayContaining(['agents.accounts', 'agents.add-login', 'token-usage.machine', 'agents.default-agent']))
    expect(ids('token usage')).toContain('token-usage.machine')
    expect(ids('Usage')[0]).toBe('token-usage.machine')
    expect(ids('models')).toContain('agents.default-agent')
  })
})

describe('Settings: K2 Server becomes Access and Network (0.45.7)', () => {
  it('group order: Language Models, Access, Network, Editors / Fonts, Logs, Cores, 3rd Party Bots (Gardens is a page, 0.46.3)', () => {
    expect(titles(settingsNav({ zenGardens: true }))).toEqual([
      'Language Models', 'Access', 'Network', 'Editors / Fonts', 'Logs', 'Cores', '3rd Party Bots',
    ])
    expect(titles(settingsNav({ zenGardens: false }))).toEqual([
      'Language Models', 'Access', 'Network', 'Editors / Fonts', 'Logs', 'Cores', '3rd Party Bots',
    ])
  })

  it('Access and Network hold the old K2 Server pages; every old id is still a leaf exactly once', () => {
    for (const zenGardens of [true, false]) {
      const nav = settingsNav({ zenGardens })
      // Permission Cards (Beta, 0.46.3, prd-agent-settings-cards-v1 §2.1) closes Access.
      expect(group(nav, 'Access').items.map((i) => i.id)).toEqual(['k2-access', 'people', 'user-templates', 'api-tokens', 'vault', 'permission-cards'])
      expect(group(nav, 'Network').items.map((i) => i.id)).toEqual(
        zenGardens
          ? ['k2-connect', 'domains', 'connections', 'companion', 'homes-sync']
          : ['k2-connect', 'domains', 'connections', 'companion', 'homes-sync', 'data-sources'],
      )
      const ids = leafIds(nav)
      for (const id of [
        'k2-access', 'people', 'user-templates', 'domains', 'k2-connect', 'connections',
        'api-tokens', 'vault', 'local-grokbots', 'companion', 'access-audit',
      ] as const) {
        expect(ids.filter((x) => x === id).length, id).toBe(1)
      }
      // External Data: its own leaf only without Gardens (web, Windows);
      // with Gardens it lives in each Garden's pane (0.46.3).
      expect(ids.filter((x) => x === 'data-sources').length).toBe(zenGardens ? 0 : 1)
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
    expect(group(nav, 'Cores').items.map((i) => i.id)).toEqual(['email-hosting', 'email-notifications', 'sew', 'compute', 'data', 'skin-access', 'app-approvals'])
    expect(SECTION_LABELS['email-notifications']).toBe('Email Notifications')
    // Chat Hosting is a real page since S32 K27 (was a SOON placeholder).
    const chat = group(nav, 'Cores').items[2]
    expect(chat).toEqual({ id: 'sew', label: 'Chat Hosting' })
    expect(SECTION_LABELS.sew).toBe('Chat Hosting')
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

  it('App Approvals (0.46.1 door) sits right under Apps in Cores, opens, searches and is labelled', () => {
    for (const zenGardens of [true, false]) {
      const items = group(settingsNav({ zenGardens }), 'Cores').items
      const apps = items.findIndex((i) => i.id === 'skin-access')
      expect(items[apps + 1]).toEqual({ id: 'app-approvals', label: 'App Approvals' })
      expect(leafIds(settingsNav({ zenGardens })).filter((x) => x === 'app-approvals').length).toBe(1)
    }
    expect(SECTION_LABELS['app-approvals']).toBe('App Approvals')
    const s = useSettingsStore.getState()
    s.openSettings('app-approvals')
    expect(useSettingsStore.getState().activeSection).toBe('app-approvals')
    s.closeSettings()
    const settingsSrc = readFileSync(join(__dirname, 'Settings.tsx'), 'utf8')
    expect(settingsSrc).toContain("activeSection === 'app-approvals'")
    expect(settingsSrc).toContain('<AppApprovalsSection />')
    expect(settingsSrc).toContain('...APP_APPROVALS_MANIFEST,')
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

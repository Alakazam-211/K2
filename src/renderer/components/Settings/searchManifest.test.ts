// Settings search index: every section's manifest reaches the palette, the
// entries are well formed, and the ranking puts the best match first.
import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { SECTION_LABELS, searchManifest, type SettingEntry } from './searchManifest'
import { GENERAL_MANIFEST } from './sections/GeneralSection'
import { PROJECTS_MANIFEST } from './sections/ProjectsSection'
import { AGENT_SKILLS_MANIFEST } from './sections/AgentSkillsSection'
import { TERMINAL_MANIFEST } from './sections/TerminalSection'
import { CODE_EDITOR_MANIFEST } from './sections/CodeEditorSettingsSection'
import { EDITORS_MANIFEST } from './sections/EditorsSection'
import { KEYBINDINGS_MANIFEST } from './sections/KeybindingsSection'
import { TIMER_MANIFEST } from './sections/TimerSection'
import { COMPANION_MANIFEST } from './sections/CompanionSection'

const here = dirname(fileURLToPath(import.meta.url))

describe('every section manifest is in the search index', () => {
  it('Settings.tsx spreads each exported *_MANIFEST into the palette entries', () => {
    const files = [
      ...readdirSync(join(here, 'sections'))
        .filter((f) => /\.tsx?$/.test(f) && !f.includes('.test.'))
        .map((f) => join(here, 'sections', f)),
      join(here, '../Projects/project-settings.ts'),
    ]
    const exported = files.flatMap((f) =>
      [...readFileSync(f, 'utf8').matchAll(/export const ([A-Z0-9_]+_MANIFEST)\b/g)].map((m) => m[1]),
    )
    // A scan that finds nothing would pass vacuously.
    expect(exported.length).toBeGreaterThan(40)
    const router = readFileSync(join(here, 'Settings.tsx'), 'utf8')
    const start = router.indexOf('const allEntries = useMemo')
    const end = router.indexOf('    [],\n  )', start)
    expect(start).toBeGreaterThan(-1)
    expect(end).toBeGreaterThan(start)
    const index = router.slice(start, end)
    // `...X_MANIFEST`, or `...(shown() ? X_MANIFEST : [])` for a page that
    // only some servers have.
    const missing = exported.filter((name) => !index.includes(`...${name},`) && !index.includes(`? ${name} : []`))
    expect(missing).toEqual([])
  })
})

describe('section manifests', () => {
  const manifests: Record<string, SettingEntry[]> = {
    GENERAL_MANIFEST,
    PROJECTS_MANIFEST,
    AGENT_SKILLS_MANIFEST,
    TERMINAL_MANIFEST,
    CODE_EDITOR_MANIFEST,
    EDITORS_MANIFEST,
    KEYBINDINGS_MANIFEST,
    TIMER_MANIFEST,
    COMPANION_MANIFEST,
  }

  it('each is non-empty, and every entry has an id, a label and a labelled section', () => {
    for (const [name, entries] of Object.entries(manifests)) {
      expect(entries.length, name).toBeGreaterThan(0)
      for (const e of entries) {
        expect(e.id, `${name}: ${JSON.stringify(e)}`).toMatch(/^[a-z0-9-]+\.[A-Za-z0-9.-]+$/)
        expect(e.label.trim(), e.id).not.toBe('')
        expect(SECTION_LABELS[e.section], `${e.id} section ${e.section}`).toBeTruthy()
      }
    }
  })
})

describe('setting ids', () => {
  // Every module the scan above reads, loaded for real, so the check sees
  // the same *_MANIFEST arrays the palette spreads.
  const modules = {
    ...import.meta.glob<Record<string, unknown>>(['./sections/*.ts', './sections/*.tsx', '!./sections/*.test.*'], { eager: true }),
    ...import.meta.glob<Record<string, unknown>>('../Projects/project-settings.ts', { eager: true }),
  }

  it('are unique across every section manifest, so a search result jumps to one row', () => {
    const owners = new Map<string, string[]>()
    let manifestCount = 0
    for (const [file, mod] of Object.entries(modules)) {
      for (const [name, value] of Object.entries(mod)) {
        if (!name.endsWith('_MANIFEST')) continue
        expect(Array.isArray(value), `${file} ${name}`).toBe(true)
        manifestCount += 1
        for (const e of value as SettingEntry[]) {
          owners.set(e.id, [...(owners.get(e.id) ?? []), name])
        }
      }
    }
    // A glob that loads nothing would pass vacuously.
    expect(manifestCount).toBeGreaterThan(40)
    const duplicates = [...owners].filter(([, names]) => names.length > 1)
    expect(duplicates).toEqual([])
  })
})

describe('searchManifest ranking', () => {
  const entries: SettingEntry[] = [
    { id: 'a.desc', section: 'terminal', label: 'Cursor Style', description: 'How the font looks' },
    { id: 'a.keyword', section: 'terminal', label: 'Typeface', keywords: ['font'] },
    { id: 'a.contains', section: 'terminal', label: 'Editor font' },
    { id: 'a.prefix', section: 'terminal', label: 'Font Size' },
    { id: 'a.exact', section: 'terminal', label: 'Font' },
    { id: 'a.none', section: 'timer', label: 'Timezone' },
  ]
  const ids = (q: string): string[] => searchManifest(entries, q).map((e) => e.id)

  it('exact label, then prefix, contains, keyword, description; misses are left out', () => {
    expect(ids('font')).toEqual(['a.exact', 'a.prefix', 'a.contains', 'a.keyword', 'a.desc'])
    expect(ids('FONT ')).toEqual(ids('font'))
  })

  it('every word of a multi-word query must appear somewhere', () => {
    expect(ids('style looks')).toEqual(['a.desc'])
    expect(ids('style nowhere')).toEqual([])
  })

  it('an empty query lists everything in manifest order', () => {
    expect(ids('  ')).toEqual(entries.map((e) => e.id))
  })
})

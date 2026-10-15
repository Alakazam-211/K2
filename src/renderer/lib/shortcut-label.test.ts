import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { metaChordLabel, withChord } from './shortcut-label'
import { defaultAgentLaunchCopy } from '@/components/Settings/sections/AgentsSection'
import { hireConfirmCopy } from '@/components/Settings/sections/ApiTokensSection'

describe('metaChordLabel', () => {
  it('keeps the macOS glyphs unchanged', () => {
    expect(metaChordLabel('Meta+T', 'mac')).toBe('⌘T')
    expect(metaChordLabel('Meta+N', 'mac')).toBe('⌘N')
    expect(metaChordLabel('Shift+Meta+T', 'mac')).toBe('⇧⌘T')
    expect(metaChordLabel('Meta+,', 'mac')).toBe('⌘,')
  })

  it('names Super on Linux, the key the metaKey handlers read', () => {
    expect(metaChordLabel('Meta+T', 'linux')).toBe('Super+T')
    expect(metaChordLabel('Meta+N', 'linux')).toBe('Super+N')
    expect(metaChordLabel('Shift+Meta+T', 'linux')).toBe('Super+Shift+T')
    expect(metaChordLabel('Meta+,', 'linux')).toBe('Super+,')
    for (const os of ['linux', 'other'] as const) {
      expect(metaChordLabel('Shift+Meta+T', os)).not.toMatch(/[⌘⇧⌥⌃]/)
    }
  })

  it('shows no chord on Windows, which keeps Win+T/N/, for itself', () => {
    expect(metaChordLabel('Meta+T', 'windows')).toBeNull()
    expect(withChord('Settings', 'Meta+,', 'windows')).toBe('Settings')
    expect(withChord('Settings', 'Meta+,', 'mac')).toBe('Settings (⌘,)')
    expect(withChord('Settings', 'Meta+,', 'linux')).toBe('Settings (Super+,)')
  })
})

describe('platform copy', () => {
  it('Default AI Agent names the real chord', () => {
    expect(defaultAgentLaunchCopy('mac')).toBe('Launched with ⇧⌘T or from the assistant')
    expect(defaultAgentLaunchCopy('linux')).toBe('Launched with Super+Shift+T or from the assistant')
    expect(defaultAgentLaunchCopy('windows')).toBe('Launched from the tab + menu or the assistant')
  })

  it('hire confirm offers Touch ID only on a Mac', () => {
    expect(hireConfirmCopy('mac')).toContain('Touch ID')
    for (const os of ['linux', 'windows', 'other'] as const) {
      expect(hireConfirmCopy(os)).toBe('Creating it asks for your password to confirm.')
    }
  })

  it('no reported site hard-codes a Mac chord any more', () => {
    const src = (p: string) => readFileSync(resolve(__dirname, '..', p), 'utf8')
    expect(src('components/TabBar/TabBar.tsx')).not.toMatch(/shortcut: '[⌘⇧]/)
    expect(src('components/TopBar/PageTabs.tsx')).not.toContain('Settings (⌘,)')
    expect(src('components/Settings/sections/ProjectsSection.tsx')).not.toContain('The agent ⇧⌘T launches')
    expect(src('components/Settings/sections/AgentsSection.tsx')).not.toContain("'Launched with ⇧⌘T")
  })
})

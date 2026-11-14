// @vitest-environment jsdom
//
// The curated Home group icons: every name is a real lucide-react 1.73.0
// icon (the component it imports draws `lucide-<name>`), none is a brand or
// company logo, names are storable, and the search finds by name and word.

import { describe, expect, it } from 'vitest'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import * as lucide from 'lucide-react'
import { GROUP_ICONS, groupIcon, searchGroupIcons } from './group-icons'
import { GROUP_ICON_NAME_RE } from './group-icon-name'

/** Brands, companies and products (Lucide's old brand icons and their
 *  kin). None may ever be offered as a group icon. */
const BRAND_DENYLIST = [
  'apple',
  'android',
  'chrome',
  'chromium',
  'codepen',
  'codesandbox',
  'dribbble',
  'facebook',
  'figma',
  'framer',
  'github',
  'gitlab',
  'instagram',
  'linkedin',
  'pocket',
  'rail-symbol',
  'slack',
  'trello',
  'twitch',
  'twitter',
  'youtube',
]

function pascal(name: string): string {
  return name
    .split('-')
    .map((p) => p.charAt(0).toUpperCase() + p.slice(1))
    .join('')
}

describe('the curated group icons', () => {
  it('is a curated set, not the whole of Lucide', () => {
    expect(GROUP_ICONS.length).toBeGreaterThanOrEqual(150)
    expect(GROUP_ICONS.length).toBeLessThanOrEqual(250)
    expect(new Set(GROUP_ICONS.map((d) => d.name)).size).toBe(GROUP_ICONS.length)
  })

  it('every name is storable and resolves to that lucide-react export, which draws that icon', () => {
    const exports = lucide as unknown as Record<string, unknown>
    for (const d of GROUP_ICONS) {
      expect([d.name, GROUP_ICON_NAME_RE.test(d.name)]).toEqual([d.name, true])
      expect([d.name, exports[pascal(d.name)] === d.Icon]).toEqual([d.name, true])
      const svg = renderToStaticMarkup(createElement(d.Icon))
      expect([d.name, svg.includes(`lucide-${d.name}`)]).toEqual([d.name, true])
    }
  })

  it('holds every picker icon K2 Design’s mock uses (its names were taken from lucide-react 1.48)', () => {
    const mock = `house briefcase palette shopping-cart code terminal book-open music camera film gamepad-2 heart star
      sun moon cloud leaf tree-pine mountain flower-2 coffee utensils plane car bike rocket globe map compass flag bell
      calendar clock inbox mail message-square phone users user baby dog cat wrench hammer lightbulb flask-conical
      graduation-cap dumbbell wallet piggy-bank chart-line folder archive package gift shield lock key zap flame
      droplet anchor sparkles bookmark pencil pen-line brush layers server cpu laptop smartphone headphones mic video
      store building tent umbrella trophy target puzzle bot`.split(/\s+/)
    expect(mock.length).toBe(83)
    const names = new Set(GROUP_ICONS.map((d) => d.name))
    expect(mock.filter((n) => !names.has(n))).toEqual([])
  })

  it('holds no brand or company logo', () => {
    const names = GROUP_ICONS.map((d) => d.name)
    for (const brand of BRAND_DENYLIST) {
      expect(names.filter((n) => n === brand || n.startsWith(`${brand}-`))).toEqual([])
    }
  })

  it('looks icons up by stored name; unknown or none is null (the dot)', () => {
    expect(groupIcon('briefcase')?.Icon).toBe((lucide as unknown as Record<string, unknown>).Briefcase)
    expect(groupIcon('not-an-icon')).toBeNull()
    expect(groupIcon(null)).toBeNull()
    expect(groupIcon(undefined)).toBeNull()
    expect(groupIcon('')).toBeNull()
  })

  it('search matches the name’s words and keywords by prefix; empty returns all', () => {
    expect(searchGroupIcons('').length).toBe(GROUP_ICONS.length)
    expect(searchGroupIcons('rock').map((d) => d.name)).toContain('rocket')
    expect(searchGroupIcons('pin').map((d) => d.name)).toEqual(expect.arrayContaining(['map-pin', 'pin']))
    expect(searchGroupIcons('GAME').map((d) => d.name)).toContain('gamepad-2')
    // `house` is found by its keyword "home".
    expect(searchGroupIcons('home').map((d) => d.name)).toContain('house')
    expect(searchGroupIcons('zzzz-nothing')).toEqual([])
  })
})

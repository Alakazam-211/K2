// prd-garden-access-v1 (A5): the one typed client's parsers and words.
// Pure; no daemon. Bad shapes throw (the page shows the error, never guesses).
import { describe, expect, it } from 'vitest'
import {
  gardenAccessSetting,
  gardenActorWords,
  gardenEditorAddedWords,
  gardenHistoryLine,
  gardenListFacts,
  gardenWidgetSignerWords,
  parseDeletedGardens,
  parseGardenAccessList,
  parseGardenHistory,
  parseGardenWidgets,
  parseLocalAgents,
  type GardenHistoryItem,
} from './garden-access-api'

const AT = Date.UTC(2026, 9, 12, 12, 0, 0) / 1000
const NOW = new Date(Date.UTC(2026, 9, 22, 12, 0, 0))

function item(over: Partial<GardenHistoryItem>): GardenHistoryItem {
  return {
    id: 'e',
    at: AT,
    kind: 'changed',
    source: 'event',
    actor: { kind: 'owner', id: null, handle: null, name: null, color: null },
    widget: null,
    detail: {},
    snapshot: null,
    summary: null,
    ...over,
  }
}

describe('the switch', () => {
  it('reads true/false, null when absent, and throws on anything else', () => {
    expect(gardenAccessSetting({ gardenAccess: true })).toBe(true)
    expect(gardenAccessSetting({ gardenAccess: false })).toBe(false)
    expect(gardenAccessSetting({ theme: 'dark' })).toBeNull()
    expect(() => gardenAccessSetting({ gardenAccess: 'yes' })).toThrow('"gardenAccess" is not true or false')
  })
})

describe('the access list', () => {
  it('parses editors and the maker', () => {
    const l = parseGardenAccessList({
      ok: true,
      garden: 'g-2',
      name: 'Fleet Ops',
      made: { agent: 'p-bill', handle: 'billing', name: 'Billing', at: AT },
      editors: [{ agent: 'p-ana', handle: 'ana', name: 'Ana', color: '#3366ff', addedBy: 'migration', addedAt: AT }],
    })
    expect(l).toEqual({
      garden: 'g-2',
      name: 'Fleet Ops',
      made: { agent: 'p-bill', handle: 'billing', name: 'Billing', at: AT },
      editors: [{ agent: 'p-ana', handle: 'ana', name: 'Ana', color: '#3366ff', addedBy: 'migration', addedAt: AT }],
    })
    expect(gardenEditorAddedWords(l.editors[0], NOW)).toBe(`added automatically at upgrade · ${new Date(AT * 1000).getDate()} Oct`)
  })

  it('throws on a missing list, an unknown addedBy or a missing time', () => {
    expect(() => parseGardenAccessList({ garden: 'g' })).toThrow('no "editors"')
    expect(() =>
      parseGardenAccessList({ garden: 'g', editors: [{ agent: 'a', addedBy: 'agent', addedAt: 1 }] }),
    ).toThrow('is not owner, card or migration')
    expect(() => parseGardenAccessList({ garden: 'g', editors: [{ agent: 'a', addedBy: 'owner' }] })).toThrow(
      'editors[0].addedAt is missing',
    )
  })

  it('sidebar facts come from the Garden list rows (absent = no facts)', () => {
    const f = gardenListFacts({
      gardens: [
        { id: 'g-1', name: 'Garden 1', editors: 0, madeBy: null },
        { id: 'g-2', name: 'Fleet', editors: 2, madeBy: { agent: 'p-bill', handle: 'billing', name: 'Billing' } },
        { id: 'g-3', name: 'Old daemon' },
      ],
    })
    expect(f.get('g-1')).toEqual({ editors: 0, madeBy: null })
    expect(f.get('g-2')).toEqual({ editors: 2, madeBy: { agent: 'p-bill', handle: 'billing', name: 'Billing' } })
    expect(f.get('g-3')).toEqual({ editors: null, madeBy: null })
  })
})

describe('history', () => {
  it('parses items, the cursor and the unsigned versions; throws on an unknown actor or source', () => {
    const p = parseGardenHistory({
      ok: true,
      items: [{ id: 7, at: AT, kind: 'signed', source: 'stamp', actor: { kind: 'agent', id: 'p-ana', name: 'Ana' }, widget: 'w', detail: { hash: 'abc' } }],
      next: 'c',
      unsigned: [{ widget: 'w', hash: 'h', gardens: ['g-1'] }],
    })
    expect(p.items[0].id).toBe('7')
    expect(p.items[0].source).toBe('stamp')
    expect(p.next).toBe('c')
    expect(p.unsigned).toEqual([{ widget: 'w', hash: 'h', gardens: ['g-1'] }])
    expect(() => parseGardenHistory({ items: [{ id: 'x', at: 1, kind: 'k', actor: { kind: 'robot' } }] })).toThrow('is not an actor kind')
    expect(() => parseGardenHistory({ items: [{ id: 'x', at: 1, kind: 'k', source: 'gossip', actor: { kind: 'k2' } }] })).toThrow(
      'source "gossip" is not known',
    )
    expect(() => parseGardenHistory({})).toThrow('no "items"')
  })

  it('says who and what in the modal’s words', () => {
    expect(gardenActorWords({ kind: 'shell', id: 'p', handle: 'billing', name: 'Billing', color: null })).toBe(
      'A terminal in Billing’s room',
    )
    expect(gardenActorWords({ kind: 'disk', id: null, handle: null, name: null, color: null })).toBe('Outside K2')
    expect(gardenHistoryLine(item({ kind: 'access_added', actor: { kind: 'migration', id: null, handle: null, name: null, color: null }, detail: { name: 'ops-bot' } }))).toBe(
      'ops-bot was added automatically at upgrade',
    )
    expect(gardenHistoryLine(item({ kind: 'access_added', detail: { name: 'Ana' } }))).toBe('You added Ana')
    expect(gardenHistoryLine(item({ kind: 'renamed', detail: { old: 'A', new: 'B' } }))).toBe('You renamed it from “A” to “B”')
    expect(gardenHistoryLine(item({ kind: 'delete_asked', actor: { kind: 'agent', id: 'p', handle: 'billing', name: 'Billing', color: null } }))).toBe(
      'Billing asked to delete it',
    )
    expect(gardenHistoryLine(item({ source: 'snapshot', widget: 'w' }))).toBe('You saved a version of w')
    expect(gardenHistoryLine(item({ summary: 'K2’s own words' }))).toBe('You K2’s own words')
  })
})

describe('widgets, deleted Gardens, agents', () => {
  it('widget lines carry the last signer and per-Garden signed state', () => {
    const [w] = parseGardenWidgets({
      widgets: [
        {
          name: 'w',
          builtin: false,
          signedBy: { kind: 'agent', handle: 'ana', name: 'Ana', at: AT },
          changedAt: AT,
          placements: [
            { garden: 'g-1', signed: true },
            { garden: 'g-2', signed: false },
          ],
        },
      ],
    })
    expect(w.gardens).toEqual(['g-1', 'g-2'])
    expect(w.signedIn).toEqual({ 'g-1': true, 'g-2': false })
    expect(gardenWidgetSignerWords(w, NOW)).toMatch(/^last signed by Ana · \d+ Oct$/)
    expect(gardenWidgetSignerWords({ signedBy: null, changedAt: AT }, NOW)).toMatch(/^last changed \d+ Oct$/)
    expect(gardenWidgetSignerWords({ signedBy: null, changedAt: null }, NOW)).toBe('')
  })

  it('deleted Gardens and local agents parse; bad bodies throw', () => {
    expect(parseDeletedGardens({ gardens: [{ id: 'g-old', name: 'Old', deletedAt: AT, deletedBy: { kind: 'owner' } }] })).toEqual([
      { id: 'g-old', name: 'Old', deletedAt: AT, deletedBy: { kind: 'owner', id: null, handle: null, name: null, color: null } },
    ])
    expect(() => parseDeletedGardens({})).toThrow('no "gardens"')
    expect(parseLocalAgents([{ id: 'p', name: 'Ana', path: '/w/ana', color: '#fff' }])).toEqual([
      { id: 'p', name: 'Ana', path: '/w/ana', color: '#fff', iconUrl: null, focusGroupId: null },
    ])
    expect(() => parseLocalAgents({})).toThrow('projects/list did not return a list')
  })
})

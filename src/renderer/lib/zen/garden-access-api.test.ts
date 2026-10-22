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

  it('reads A1’s raw event rows too: actor labels, numeric ids and cursors, `workspace` in the detail', () => {
    const p = parseGardenHistory({
      items: [
        { id: 12, gardenId: 'g-2', widget: null, at: AT, actor: 'migration', kind: 'access_added', detail: { workspace: 'ops-bot', via: 'migration' } },
        { id: 11, gardenId: 'g-2', widget: null, at: AT, actor: 'shell:p-bill', kind: 'changed', detail: {} },
      ],
      next: 11,
    })
    expect(p.next).toBe('11')
    expect(p.items[1].actor).toEqual({ kind: 'shell', id: 'p-bill', handle: null, name: null, color: null })
    expect(gardenHistoryLine(p.items[0])).toBe('ops-bot was added automatically at upgrade')
    expect(gardenHistoryLine(p.items[1])).toBe('A terminal in p-bill’s room changed the layout')
    expect(() => parseGardenHistory({ items: [{ id: 1, at: 1, kind: 'k', actor: 'robot:x' }] })).toThrow('"robot:x" is not an actor')
    expect(parseGardenAccessList({ garden: 'g', editors: [{ projectId: 'p-ana', addedBy: 'card', addedAt: AT }] }).editors[0].agent).toBe('p-ana')
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

// Real answers from a sandbox daemon (scripts/test/garden_access_smoke.sh,
// GA_SMOKE_DUMP=1, 2026-10-22): the client must read the daemon as it is.
const REAL = {
  access: {
    "editors": [],
    "garden": "g-9db6da5e",
    "locked": false,
    "made": {
      "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
      "at": 1792698343,
      "color": "#3b82f6",
      "handle": "alpha",
      "name": "alpha"
    },
    "name": "Sales board",
    "ok": true,
    "sees": "everything"
  },
  gardens: {
    "gardens": [
      {
        "createdAt": "2026-10-22T19:45:42Z",
        "editors": 0,
        "hasFile": true,
        "id": "g-2b577949",
        "index": 1,
        "locked": false,
        "madeBy": null,
        "name": "Garden 1",
        "template": "k2.texting@1",
        "theme": null
      },
      {
        "createdAt": "2026-10-22T19:45:42Z",
        "editors": 0,
        "hasFile": true,
        "id": "g-d7017e2b",
        "index": 2,
        "locked": false,
        "madeBy": null,
        "name": "Garden 2",
        "template": "k2.blank@1",
        "theme": null
      },
      {
        "createdAt": "2026-10-22T19:45:42Z",
        "editors": 0,
        "hasFile": true,
        "id": "g-5acd63f0",
        "index": 3,
        "locked": false,
        "madeBy": null,
        "name": "Diary",
        "template": "k2.diary@2",
        "theme": null
      },
      {
        "createdAt": "2026-10-22T19:45:43Z",
        "editors": 0,
        "hasFile": true,
        "id": "g-9db6da5e",
        "index": 4,
        "locked": false,
        "madeBy": {
          "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "handle": "alpha",
          "name": "alpha"
        },
        "name": "Sales board",
        "template": "k2.blank@1",
        "theme": null
      }
    ],
    "ok": true,
    "setUp": true
  },
  deleted: {
    "gardens": [
      {
        "deletedAt": 1792698344,
        "deletedBy": {
          "id": "phone",
          "kind": "login",
          "name": "phone"
        },
        "id": "g-9db6da5e",
        "name": "Sales",
        "snapshot": "20261022T194544417Z-000.toml"
      }
    ],
    "ok": true
  },
  widgets: {
    "ok": true,
    "widgets": [
      {
        "builtin": false,
        "caps": [
          "store:read",
          "store:write",
          "thread:post",
          "agents:read",
          "files:read"
        ],
        "changedAt": 1792698344,
        "description": null,
        "errors": [],
        "hash": "ac91ac34a823e2baf815fd26f9c3b59b6f7897235103d2e4c04e7151b8a4b791",
        "madeBy": {
          "handle": "alpha",
          "kind": "agent",
          "projectId": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb"
        },
        "name": "tally",
        "placements": [
          {
            "garden": "g-9db6da5e",
            "gardenName": "Sales",
            "paused": null,
            "placement": "t1",
            "signed": true
          }
        ],
        "signedBy": {
          "at": 1792698344,
          "handle": "alpha",
          "kind": "agent",
          "name": "alpha"
        },
        "state": "ok",
        "title": "Tally",
        "warnings": []
      },
      {
        "builtin": true,
        "caps": [
          "agents:read",
          "thread:read",
          "thread:post"
        ],
        "changedAt": null,
        "description": "A haunted journal with one page per agent on this computer. Turn the page to write to another; the ink writes back.",
        "errors": [],
        "hash": "d1fd852aca7e323d0ea9736508001a7709abe2a5113559547e08a17e1cf49bfb",
        "madeBy": {
          "kind": "k2"
        },
        "name": "k2:diary@2",
        "placements": [
          {
            "garden": "g-5acd63f0",
            "gardenName": "Diary",
            "paused": null,
            "placement": "diary",
            "signed": true
          }
        ],
        "signedBy": {
          "at": null,
          "kind": "k2"
        },
        "state": "ok",
        "title": "Diary",
        "warnings": []
      }
    ]
  },
  gardenHistory: {
    "garden": "g-9db6da5e",
    "items": [
      {
        "actor": {
          "kind": "owner",
          "name": "you"
        },
        "at": 1792698346,
        "detail": {
          "hash": "d6a553e9047d",
          "vouched": true
        },
        "garden": "g-9db6da5e",
        "id": "event:14",
        "kind": "signed",
        "snapshot": null,
        "source": "event",
        "widget": "tally"
      },
      {
        "actor": {
          "agent": "f3b09095-9a7f-4517-a8ba-dbd94c257ef8",
          "color": "#3b82f6",
          "handle": "bravo",
          "id": "f3b09095-9a7f-4517-a8ba-dbd94c257ef8",
          "kind": "agent",
          "name": "bravo"
        },
        "at": 1792698346,
        "detail": {
          "hash": "d6a553e9047d",
          "vouched": false
        },
        "garden": "g-9db6da5e",
        "id": "event:13",
        "kind": "signed",
        "snapshot": null,
        "source": "event",
        "widget": "tally"
      },
      {
        "actor": {
          "kind": "owner",
          "name": "you"
        },
        "at": 1792698344,
        "detail": {
          "name": "Sales"
        },
        "garden": "g-9db6da5e",
        "id": "event:12",
        "kind": "restored",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "kind": "owner",
          "name": "you"
        },
        "at": 1792698344,
        "detail": {
          "via": "garden_restore",
          "workspace": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb"
        },
        "garden": "g-9db6da5e",
        "id": "event:11",
        "kind": "access_added",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "id": "phone",
          "kind": "login",
          "name": "phone"
        },
        "at": 1792698344,
        "detail": {
          "name": "Sales"
        },
        "garden": "g-9db6da5e",
        "id": "event:10",
        "kind": "deleted",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "id": "phone",
          "kind": "login",
          "name": "phone"
        },
        "at": 1792698344,
        "detail": {
          "via": "garden_delete",
          "workspace": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb"
        },
        "garden": "g-9db6da5e",
        "id": "event:9",
        "kind": "access_removed",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "color": "#3b82f6",
          "handle": "alpha",
          "id": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "kind": "agent",
          "name": "alpha"
        },
        "at": 1792698344,
        "detail": {
          "cardId": "9515f7ef-a450-4d19-a803-1b83cf5f8180",
          "made": false,
          "request": "pr_05804fb5531d",
          "workspace": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb"
        },
        "garden": "g-9db6da5e",
        "id": "event:8",
        "kind": "delete_asked",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "agent": "f3b09095-9a7f-4517-a8ba-dbd94c257ef8",
          "color": "#3b82f6",
          "handle": "bravo",
          "id": "f3b09095-9a7f-4517-a8ba-dbd94c257ef8",
          "kind": "agent",
          "name": "bravo"
        },
        "at": 1792698344,
        "detail": {
          "cardId": "f0791211-2aee-4b80-bcb5-9c23dea86001",
          "made": false,
          "request": "pr_955ed3935e78",
          "workspace": "f3b09095-9a7f-4517-a8ba-dbd94c257ef8"
        },
        "garden": "g-9db6da5e",
        "id": "event:7",
        "kind": "access_asked",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "color": "#3b82f6",
          "handle": "alpha",
          "id": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "kind": "agent",
          "name": "alpha"
        },
        "at": 1792698344,
        "detail": {
          "file": "widgets/tally",
          "hash": "ac91ac34a823",
          "note": null,
          "written": [
            "index.html"
          ]
        },
        "garden": null,
        "id": "event:6",
        "kind": "widget_changed",
        "snapshot": null,
        "source": "event",
        "widget": "tally"
      },
      {
        "actor": {
          "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "color": "#3b82f6",
          "handle": "alpha",
          "id": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "kind": "agent",
          "name": "alpha"
        },
        "at": 1792698344,
        "detail": {
          "file": "gardens/g-9db6da5e.toml",
          "hash": "eb03407061f9",
          "note": "wider",
          "snapshot": "20261022T194543844Z-000.toml"
        },
        "garden": "g-9db6da5e",
        "id": "event:5",
        "kind": "changed",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "color": "#3b82f6",
          "handle": "alpha",
          "id": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "kind": "agent",
          "name": "alpha"
        },
        "at": 1792698344,
        "detail": {
          "to": "Sales"
        },
        "garden": "g-9db6da5e",
        "id": "event:4",
        "kind": "renamed",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "kind": "owner",
          "name": "you"
        },
        "at": 1792698343,
        "detail": {
          "cardId": "ca8398d7-3746-40b9-af67-9eb55f8bbdce",
          "via": "card",
          "workspace": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb"
        },
        "garden": "g-9db6da5e",
        "id": "event:3",
        "kind": "access_added",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "color": "#3b82f6",
          "handle": "alpha",
          "id": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "kind": "agent",
          "name": "alpha"
        },
        "at": 1792698343,
        "detail": {
          "cardId": "ca8398d7-3746-40b9-af67-9eb55f8bbdce",
          "made": true,
          "request": "pr_5f94896bef20",
          "workspace": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb"
        },
        "garden": "g-9db6da5e",
        "id": "event:2",
        "kind": "access_asked",
        "snapshot": null,
        "source": "event",
        "widget": null
      },
      {
        "actor": {
          "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "color": "#3b82f6",
          "handle": "alpha",
          "id": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "kind": "agent",
          "name": "alpha"
        },
        "at": 1792698343,
        "detail": {
          "name": "Sales board",
          "page": true,
          "template": "k2.blank@1",
          "widgets": [
            "tally"
          ]
        },
        "garden": "g-9db6da5e",
        "id": "event:1",
        "kind": "made",
        "snapshot": null,
        "source": "event",
        "widget": null
      }
    ],
    "name": "Sales",
    "next": null,
    "ok": true,
    "unsigned": [],
    "widgets": [
      "tally"
    ]
  },
  gardenHistoryUnsigned: { next: null, unsigned: [
    {
      "gardens": [
        "g-9db6da5e"
      ],
      "hash": "d6a553e9047d",
      "widget": "tally"
    }
  ] },
  widgetHistory: {
    "gardens": [
      {
        "id": "g-9db6da5e",
        "name": "Sales"
      }
    ],
    "items": [
      {
        "actor": {
          "agent": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "color": "#3b82f6",
          "handle": "alpha",
          "id": "5ae459d1-9453-4ecd-8321-74e5ccd5c5cb",
          "kind": "agent",
          "name": "alpha"
        },
        "at": 1792698344,
        "detail": {
          "file": "widgets/tally",
          "hash": "ac91ac34a823",
          "note": null,
          "written": [
            "index.html"
          ]
        },
        "garden": null,
        "id": "event:6",
        "kind": "widget_changed",
        "snapshot": null,
        "source": "event",
        "widget": "tally"
      }
    ],
    "next": null,
    "ok": true,
    "unsigned": [
      {
        "gardens": [
          "g-9db6da5e"
        ],
        "hash": "d6a553e9047d",
        "widget": "tally"
      }
    ],
    "widget": "tally"
  },
}
const ALPHA = '5ae459d1-9453-4ecd-8321-74e5ccd5c5cb'
const BRAVO = 'f3b09095-9a7f-4517-a8ba-dbd94c257ef8'


describe('real daemon answers (the A6 smoke)', () => {
  it('the access list, the sidebar facts and Deleted Gardens', () => {
    const l = parseGardenAccessList(REAL.access)
    expect(l.made).toEqual({ agent: ALPHA, handle: 'alpha', name: 'alpha', at: 1792698343 })
    expect(l.editors).toEqual([])
    const facts = gardenListFacts(REAL.gardens)
    expect(facts.get('g-9db6da5e')).toEqual({ editors: 0, madeBy: { agent: ALPHA, handle: 'alpha', name: 'alpha' } })
    expect(facts.get('g-2b577949')).toEqual({ editors: 0, madeBy: null })
    const del = parseDeletedGardens(REAL.deleted)
    expect(del).toEqual([
      { id: 'g-9db6da5e', name: 'Sales', deletedAt: 1792698344, deletedBy: { kind: 'login', id: 'phone', handle: null, name: 'phone', color: null } },
    ])
    expect(gardenActorWords(del[0].deletedBy!)).toBe('phone')
  })

  it('widget lines: the last signer, the change time and the gate per Garden', () => {
    const w = parseGardenWidgets(REAL.widgets)
    expect(w.map((x) => x.widget)).toEqual(['tally', 'k2:diary@2'])
    expect(w[0]).toMatchObject({
      title: 'Tally',
      builtin: false,
      signedBy: { kind: 'agent', handle: 'alpha', name: 'alpha', at: 1792698344 },
      changedAt: 1792698344,
      signedIn: { 'g-9db6da5e': true },
      gardens: ['g-9db6da5e'],
    })
    expect(w[1]).toMatchObject({ builtin: true, signedBy: { kind: 'k2', handle: null, name: null, at: null }, signedIn: { 'g-5acd63f0': true } })
    expect(gardenWidgetSignerWords(w[0], NOW)).toBe('last signed by alpha · 22 Oct')
  })

  it('History pages parse, and each line reads in words once agents are named', () => {
    const page = parseGardenHistory(REAL.gardenHistory)
    expect(page.next).toBeNull()
    expect(page.items.map((i) => i.id)).toEqual(REAL.gardenHistory.items.map((i) => i.id))
    expect(page.items.every((i) => i.source === 'event')).toBe(true)
    const lines = page.items.map(gardenHistoryLine)
    expect(lines).toEqual([
      'You signed tally',
      'bravo signed tally',
      'You restored it',
      'You added an agent',
      'phone deleted it',
      'phone removed an agent',
      'alpha asked to delete it',
      'bravo asked to edit',
      'alpha changed tally',
      'alpha changed the layout',
      'alpha renamed it to “Sales”',
      'You added an agent',
      'alpha asked to edit',
      'alpha made this Garden',
    ])
    // A bare workspace id never shows; the detail's name does when sent.
    expect(lines.some((l) => l.includes(ALPHA) || l.includes(BRAVO))).toBe(false)
    const added = page.items[3]
    expect(gardenHistoryLine({ ...added, detail: { ...added.detail, name: 'alpha' } })).toBe('You added alpha')
    const uns = parseGardenHistory({ items: [], ...REAL.gardenHistoryUnsigned })
    expect(uns.unsigned).toEqual([{ widget: 'tally', hash: 'd6a553e9047d', gardens: ['g-9db6da5e'] }])
    const wh = parseGardenHistory(REAL.widgetHistory)
    expect(wh.items.map(gardenHistoryLine)).toEqual(['alpha changed tally'])
    expect(wh.unsigned).toEqual([{ widget: 'tally', hash: 'd6a553e9047d', gardens: ['g-9db6da5e'] }])
  })
})

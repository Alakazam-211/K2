// Rosson 2026-10-08 — New Garden's catalog helpers: the cards' order, the
// one consent sentence, and the create click's grant (this computer's
// agents only; a remote server's agent is never bound).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// No daemon here: anything that asks fails loudly.
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { useHomesStore, type Home } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { parseZenTemplates } from './zen-templates'
import {
  zenCatalogConsent,
  zenCatalogFixedScopeFor,
  zenCatalogGrantRequest,
  zenNewGardenCards,
} from './zen-new-garden'
import { __resetZenScopeForTests } from './zen-custom-scope'

const RAW = {
  ok: true,
  templates: [
    { id: 'k2.texting@1', short: 'texting', label: 'Start with the default', description: 'd', section: 'start', needsGrant: null, newUsers: true },
    { id: 'k2.blank@1', short: 'blank', label: 'Start empty and ask my agent', description: 'e', section: 'start', needsGrant: null, newUsers: true },
    {
      id: 'k2.diary@1',
      short: 'diary',
      label: 'Diary',
      description: 'A haunted journal.',
      section: 'catalog',
      needsGrant: {
        widget: 'k2:diary@1',
        caps: ['agents:read', 'thread:read', 'thread:post'],
        scope: 'local',
        consent: 'Diary can read your agents on this computer and post to their Threads.',
      },
      newUsers: true,
    },
    // An older daemon's row: no scope, no consent.
    {
      id: 'k2.board@1',
      short: 'board',
      label: 'Board',
      description: 'f',
      section: 'catalog',
      needsGrant: { widget: 'k2:board@1', caps: ['agents:read', 'presence:read'] },
      newUsers: false,
    },
  ],
}

const HOME: Home = {
  id: 'home-x',
  name: 'X',
  rows: [
    { address: 'alice::local', workspaceId: 'w1', label: 'Alice' },
    { address: 'julie::scout.k2.dev', workspaceId: 'w2', label: 'Julie' },
    { address: 'bob::local', workspaceId: 'w3', label: 'Bob' },
  ],
}

let saved: Home[]
beforeEach(() => {
  saved = useHomesStore.getState().homes
  useHomesStore.setState({ homes: [HOME], selectedId: HOME.id })
  useConnectHostStore.setState({ hosts: [], activeHost: 'local' })
  __resetZenScopeForTests()
})
afterEach(() => useHomesStore.setState({ homes: saved }))

describe('New Garden catalog helpers', () => {
  it('parses a catalog grant’s fixed scope and consent; an older row has neither', () => {
    const list = parseZenTemplates(RAW)
    expect(list.find((t) => t.short === 'diary')?.needsGrant).toEqual({
      widget: 'k2:diary@1',
      caps: ['agents:read', 'thread:read', 'thread:post'],
      scope: 'local',
      consent: 'Diary can read your agents on this computer and post to their Threads.',
    })
    expect(list.find((t) => t.short === 'board')?.needsGrant).toEqual({
      widget: 'k2:board@1',
      caps: ['agents:read', 'presence:read'],
      scope: null,
      consent: null,
    })
  })

  it('cards: the catalog first (the Diary leads), then the starts', () => {
    expect(zenNewGardenCards(parseZenTemplates(RAW)).map((t) => t.short)).toEqual(['diary', 'board', 'texting', 'blank'])
  })

  it('the consent sentence: the catalog’s own, else K2’s words from the caps; none for a start', () => {
    const list = parseZenTemplates(RAW)
    const by = (s: string) => list.find((t) => t.short === s) ?? (() => { throw new Error(s) })()
    expect(zenCatalogConsent(by('diary'))).toBe('Diary can read your agents on this computer and post to their Threads.')
    expect(zenCatalogConsent(by('board'))).toBe('Board can read your agents on this computer and see who is with them.')
    expect(zenCatalogConsent(by('texting'))).toBeNull()
  })

  it('the create click’s grant: this computer only, the local agents now, Sending on when it posts', () => {
    const list = parseZenTemplates(RAW)
    const diary = list.find((t) => t.short === 'diary')
    if (!diary) throw new Error('no diary')
    expect(zenCatalogGrantRequest(diary)).toEqual({
      scope: { server: 'local' },
      sending: true,
      entries: [
        { server: 'local', room: 'alice' },
        { server: 'local', room: 'bob' },
      ],
    })
    const board = list.find((t) => t.short === 'board')
    if (!board) throw new Error('no board')
    expect(zenCatalogGrantRequest(board)?.sending).toBe(false)
    expect(zenCatalogGrantRequest(list[0])).toBeNull()
  })

  it('a k2: widget the catalog fixes is reviewed at this computer’s scope; nothing else is', () => {
    const list = parseZenTemplates(RAW)
    expect(zenCatalogFixedScopeFor('k2:diary@1', list)).toEqual({ server: 'local' })
    expect(zenCatalogFixedScopeFor('k2:board@1', list)).toBeNull()
    expect(zenCatalogFixedScopeFor('my-diary', list)).toBeNull()
  })
})

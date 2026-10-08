// Rosson 2026-10-08 — New Garden's catalog helpers: the cards' order, and
// no permissions (a catalog Garden's row carries nothing to allow; an older
// daemon's `needsGrant` is ignored).
import { describe, expect, it, vi } from 'vitest'

// No daemon here: anything that asks fails loudly.
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { parseZenTemplates } from './zen-templates'
import { zenNewGardenCards } from './zen-new-garden'

const RAW = {
  ok: true,
  templates: [
    { id: 'k2.texting@1', short: 'texting', label: 'Start with the default', description: 'd', section: 'start', newUsers: true },
    { id: 'k2.blank@1', short: 'blank', label: 'Start empty and ask my agent', description: 'e', section: 'start', newUsers: true },
    { id: 'k2.diary@1', short: 'diary', label: 'Diary', description: 'A haunted journal.', section: 'catalog', newUsers: true },
    // A row from a build that still sent a grant: ignored.
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

describe('New Garden catalog helpers', () => {
  it('cards: the catalog first (the Diary leads), then the starts', () => {
    expect(zenNewGardenCards(parseZenTemplates(RAW)).map((t) => t.short)).toEqual(['diary', 'board', 'texting', 'blank'])
  })

  it('a template row carries nothing to allow', () => {
    for (const t of parseZenTemplates(RAW)) {
      expect(t, t.short).not.toHaveProperty('needsGrant')
      expect(Object.keys(t).sort(), t.short).toEqual(['description', 'id', 'label', 'newUsers', 'section', 'short'])
    }
  })
})

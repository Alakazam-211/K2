// prd-zen-user-widgets-v2 TUW3.7 (Conversation follows a custom widget,
// UW40) and UW48/UWB7 (a custom widget's rows are its granted scope only,
// never the window's Home or a loose view).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

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
import { useProjectsStore } from '@/stores/projects'
import { parseZenGet } from './zen-page'
import { __resetZenDataForTests, openZenConversation, zenAgentRows, zenViewFor } from './zen-data'

const WORK: Home = {
  id: 'home-work',
  name: 'Work',
  rows: [
    { address: 'alice::local', workspaceId: 'w1', label: 'Alice' },
    { address: 'bob::local', workspaceId: 'w2', label: 'Bob' },
  ],
}
const PLAY: Home = { id: 'home-play', name: 'Play', rows: [{ address: 'carol::local', workspaceId: 'w3', label: 'Carol' }] }

function page(grantState: string): ReturnType<typeof parseZenGet> {
  return parseZenGet({
    ok: true,
    page: {
      template: 'k2.blank@1',
      layout: { split: [50, 50] },
      controls: [],
      widgets: [
        {
          id: 'arcade',
          kind: 'custom',
          widget: 'agent-arcade',
          column: 0,
          props: { config: {} },
          caps: ['agents:read'],
          requested: ['agents:read'],
          name: 'Arcade',
          hash: 'h',
          state: 'ok',
          grant: {
            state: grantState,
            caps: ['agents:read'],
            granted: ['agents:read'],
            scope: { home: 'home-work' },
            entries: [],
            sending: false,
            paused: null,
            grantedAt: 'x',
            widgetHash: 'h',
          },
        },
        { id: 'talk', kind: 'conversation', column: 1, props: { agents: 'arcade' }, caps: ['thread:read'], source: 'builtin' },
      ],
    },
  })
}

let saved: { homes: Home[]; selected: string }

beforeEach(() => {
  saved = { homes: useHomesStore.getState().homes, selected: useHomesStore.getState().selectedId }
  useHomesStore.setState({ homes: [WORK, PLAY], selectedId: PLAY.id })
  useConnectHostStore.setState({ activeHost: 'local', hosts: [] })
  useProjectsStore.setState({ projects: [] })
  __resetZenDataForTests()
})

afterEach(() => {
  useHomesStore.setState({ homes: saved.homes, selectedId: saved.selected })
  __resetZenDataForTests()
})

describe('TUW3.7 / UW40: a Conversation follows a custom widget', () => {
  it('the conversation shows the custom widget’s view; its pick shows there; the window Home is unchanged', async () => {
    const p = page('granted')
    const custom = zenViewFor(p, 'g-test0001', 'arcade')
    const talk = zenViewFor(p, 'g-test0001', 'talk')
    expect(custom?.source).toBe('scope')
    expect(talk?.key).toBe(custom?.key)
    expect(zenAgentRows(custom).map((r) => r.address)).toEqual(['alice::local', 'bob::local'])
    await openZenConversation(custom!, 'bob::local')
    expect(zenAgentRows(talk).find((r) => r.selected)?.address).toBe('bob::local')
    expect(useHomesStore.getState().selectedId).toBe(PLAY.id)
  })

  it('UW48: no grant (review) → no rows; never the window’s Home, never a loose any-Home view', async () => {
    const p = page('review')
    const custom = zenViewFor(p, 'g-test0001', 'arcade')
    expect(zenAgentRows(custom)).toEqual([])
    await expect(openZenConversation(custom!, 'carol::local')).rejects.toThrow(/not an agent this widget shows/)
  })
})

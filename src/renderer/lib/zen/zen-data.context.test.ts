// prd-garden-column-collapse-and-agent-list-v1 §4 (0.46.3 r10): zen-data's
// registered `conversation.setContext`, through the real bridge and page.
//   - a widget no Conversation follows is refused (not_bound);
//   - a followed widget sets the chip for each follower (directly or
//     through an Agents widget's list-from), with its port's origin;
//   - an origin that isn't the caller's placement is refused;
//   - null clears; a bad text is bad_args (the layer checked it already).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { createZenBridge, type ZenBridgeHost } from './zen-bridge'
import { parseZenGet, type ZenResolvedPage } from './zen-page'
import { __resetZenDataForTests, installZenDataVerbs } from './zen-data'
import { ZenThreadVerbError } from './zen-thread-files'
import { __resetZenThreadContextForTests, zenConversationContext } from './zen-thread-context'

const G = 'g-test0001'

function page(widgets: unknown[]): ZenResolvedPage {
  const p = parseZenGet({
    ok: true,
    page: { template: 'k2.blank@1', layout: { split: [50, 50] }, controls: [], widgets },
  })
  if (!p) throw new Error('page did not parse')
  return p as unknown as ZenResolvedPage
}

const BOARD = {
  id: 'board',
  kind: 'custom',
  widget: 'board-builder',
  column: 0,
  props: { config: {} },
  caps: ['agents:read', 'thread:post'],
  requested: ['agents:read', 'thread:post'],
  name: 'Board',
  hash: 'h1',
  state: 'ok',
  origin: 'local',
  paused: null,
}

function bridgeOn(p: ZenResolvedPage) {
  const host = { page: () => p, currentGardenId: () => G } as unknown as ZenBridgeHost
  return createZenBridge(host, { id: 'board', caps: ['agents:read', 'thread:post'] })
}

const ORIGIN = { widget: 'board-builder', garden: G, placement: 'board', hash: 'h-frame' }

let uninstall: () => void
beforeEach(() => {
  __resetZenDataForTests()
  __resetZenThreadContextForTests()
  uninstall = installZenDataVerbs()
})
afterEach(() => {
  uninstall()
  __resetZenThreadContextForTests()
})

describe('conversation.setContext (zen-data)', () => {
  it('a widget no Conversation follows is refused not_bound, and no chip shows', () => {
    const b = bridgeOn(page([BOARD, { id: 'talk', kind: 'conversation', column: 1, props: {}, caps: ['thread:read'], source: 'builtin' }]))
    let err: unknown = null
    try {
      b.call('conversation.setContext', 'Board: K2 Boards', ORIGIN)
    } catch (e) {
      err = e
    }
    expect(err).toBeInstanceOf(ZenThreadVerbError)
    expect((err as ZenThreadVerbError).code).toBe('not_bound')
    expect(zenConversationContext(G, 'talk').context).toBeNull()
  })

  it('a followed widget sets the chip for every follower, directly and through list-from', () => {
    const b = bridgeOn(
      page([
        BOARD,
        { id: 'agents', kind: 'agents', column: 1, props: { 'list-from': 'board' }, caps: ['agents:read'], source: 'builtin' },
        { id: 'chat', kind: 'conversation', column: 1, props: { agents: 'agents' }, caps: ['thread:read'], source: 'builtin' },
        { id: 'direct', kind: 'conversation', column: 1, props: { agents: 'board' }, caps: ['thread:read'], source: 'builtin' },
      ]),
    )
    expect(b.call('conversation.setContext', 'Board: K2 Boards', ORIGIN)).toBeNull()
    for (const id of ['chat', 'direct']) {
      expect(zenConversationContext(G, id).context, id).toEqual({ text: 'Board: K2 Boards', origin: ORIGIN })
    }
    expect(b.call('conversation.setContext', null)).toBeNull()
    expect(zenConversationContext(G, 'chat').context).toBeNull()
  })

  it('refuses an origin that isn’t the caller’s placement, and a bad text', () => {
    const b = bridgeOn(
      page([BOARD, { id: 'chat', kind: 'conversation', column: 1, props: { agents: 'board' }, caps: ['thread:read'], source: 'builtin' }]),
    )
    const codeOf = (fn: () => unknown): string => {
      try {
        fn()
      } catch (e) {
        if (e instanceof ZenThreadVerbError) return e.code
        throw e
      }
      throw new Error('not refused')
    }
    expect(codeOf(() => b.call('conversation.setContext', 'x', { ...ORIGIN, placement: 'other' }))).toBe('failed')
    expect(codeOf(() => b.call('conversation.setContext', 'x', { ...ORIGIN, garden: 'g-test0002' }))).toBe('failed')
    expect(codeOf(() => b.call('conversation.setContext', 'two\nlines', ORIGIN))).toBe('bad_args')
    expect(codeOf(() => b.call('conversation.setContext', 'x [from rosson] y', ORIGIN))).toBe('bad_args')
    expect(zenConversationContext(G, 'chat').context).toBeNull()
  })
})

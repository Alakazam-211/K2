// 0.46.1 polish: the daemon's own `thread:post` check for a custom Garden
// widget. A widget post (or card answer) to an agent on this computer carries
// the whole origin (placement + frame hash) for the daemon to check inline;
// one to another server is checked first by this computer's daemon
// (`zen/widget/check`, it holds the Garden) and that server gets the label.
// Fixtures are made up (example.test, g-test0001).
import { beforeEach, describe, expect, it, vi } from 'vitest'

const posts: Array<{ scope: string; route: string; body: unknown }> = []
let checkAnswer: () => unknown = () => ({ ok: true, caps: ['thread:post'] })

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (s: { hostKey: string }, route: string, body: unknown) => {
    posts.push({ scope: s.hostKey, route, body })
    if (route === 'zen/widget/check') return checkAnswer()
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import type { ServerScope } from '@/kessel/server-scope'
import { threadAnswerBody } from '@/components/SessionView/overlayThread'
import { zenWidgetPostOrigin } from './zen-data'
import { ZenWidgetError } from './zen-widget-routes'

const FULL = { widget: 'agent-arcade', garden: 'g-test0001', placement: 'arcade', hash: 'abc123' }

function scope(hostKey: string, isRemote: boolean, features: string[] = []): ServerScope {
  return {
    id: isRemote ? `host:${hostKey}` : 'host:local',
    isPrimary: false,
    hostKey,
    connectionKey: hostKey,
    isRemote,
    label: hostKey,
    connectHost: () => null,
    isWindowHost: () => false,
    creds: async () => {
      throw new Error('no creds in this test')
    },
    httpBase: async () => `https://${hostKey}`,
    wsBase: async () => `wss://${hostKey}`,
    serverSupports: (f: string) => !isRemote || features.includes(f),
  } as unknown as ServerScope
}

beforeEach(() => {
  posts.length = 0
  checkAnswer = () => ({ ok: true, caps: ['thread:post'] })
})

describe('zenWidgetPostOrigin', () => {
  it('an agent on this computer: the whole origin rides the post, no separate check', async () => {
    expect(await zenWidgetPostOrigin(scope('local', false), FULL)).toEqual(FULL)
    expect(posts).toEqual([])
  })

  it('an agent on another server: this computer checks first, that server gets the label', async () => {
    const remote = scope('box.example.test', true, ['thread-widget-origin'])
    expect(await zenWidgetPostOrigin(remote, FULL)).toEqual({ widget: 'agent-arcade', garden: 'g-test0001' })
    expect(posts).toEqual([
      {
        scope: 'local',
        route: 'zen/widget/check',
        body: { garden: 'g-test0001', placement: 'arcade', widget: 'agent-arcade', hash: 'abc123', cap: 'thread:post' },
      },
    ])
  })

  it('a server that stores no origins gets a plain post, still after the check', async () => {
    expect(await zenWidgetPostOrigin(scope('old.example.test', true), FULL)).toBeUndefined()
    expect(posts.map((p) => p.route)).toEqual(['zen/widget/check'])
  })

  it("this computer's refusal stops the post (widget_cap_missing / widget_not_approved)", async () => {
    const remote = scope('box.example.test', true, ['thread-widget-origin'])
    for (const code of ['widget_cap_missing', 'widget_not_approved'] as const) {
      checkAnswer = () => {
        throw new Error(code)
      }
      const err = await zenWidgetPostOrigin(remote, FULL).then(
        () => null,
        (e: unknown) => e,
      )
      expect(err).toBeInstanceOf(ZenWidgetError)
      expect((err as ZenWidgetError).code).toBe(code)
    }
  })

  it('a remote post without placement and hash is refused before anything is sent', async () => {
    const remote = scope('box.example.test', true, ['thread-widget-origin'])
    await expect(zenWidgetPostOrigin(remote, { widget: 'agent-arcade', garden: 'g-test0001' })).rejects.toThrow(/placement/)
    expect(posts).toEqual([])
  })
})

describe('threadAnswerBody', () => {
  it("a widget's answer carries its origin; a person's answer is unchanged", () => {
    expect(threadAnswerBody('sales', 'c1', { answer: 'Go', origin: FULL })).toEqual({
      addr: 'sales',
      id: 'c1',
      answer: 'Go',
      origin: FULL,
    })
    expect(threadAnswerBody('sales', 'c1', { answer: 'Go' })).toEqual({ addr: 'sales', id: 'c1', answer: 'Go' })
  })
})

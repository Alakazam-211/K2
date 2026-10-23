// thread-context-v1 (0.46.3 r10): the one compose send carries the Board
// context only when asked, in exactly the daemon's shape, and reads the
// daemon's `contextRefused`. A send without one is the body it always was.
import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ bodies: [] as Record<string, unknown>[], answer: {} as Record<string, unknown> }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => {
    throw new Error('unexpected GET')
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Record<string, unknown>) => {
    if (route !== 'thread/post') throw new Error(`unexpected POST ${route}`)
    h.bodies.push(body)
    return h.answer
  }),
}))

import { postThreadCompose } from './overlayThread'

const scope = {} as never

beforeEach(() => {
  h.bodies.length = 0
  h.answer = { ok: true, id: 'm1', seq: 3, kind: 'text', from: 'rosson', body: 'ship it', via: 'compose' }
})

describe('postThreadCompose and the Board context', () => {
  it('without a context the body is unchanged', async () => {
    const r = await postThreadCompose(scope, 'sales', 'ship it')
    expect(h.bodies).toEqual([{ addr: 'sales', text: 'ship it', via: 'compose' }])
    expect(r).toMatchObject({ ok: true })
    expect(r.ok && 'contextRefused' in r).toBe(false)
  })

  it('sends {text, origin} with only the known fields, and the hash only when given', async () => {
    await postThreadCompose(scope, 'sales', 'ship it', null, {
      context: { text: 'Board: K2 Boards', origin: { widget: 'board-builder', garden: 'g-1', placement: 'board', hash: 'h1', extra: 1 } as never },
    })
    await postThreadCompose(scope, 'sales', 'ship it', null, {
      context: { text: 'Board: K2 Boards', origin: { widget: 'board-builder', garden: 'g-1', placement: 'board' } },
    })
    expect(h.bodies[0].context).toEqual({
      text: 'Board: K2 Boards',
      origin: { widget: 'board-builder', garden: 'g-1', placement: 'board', hash: 'h1' },
    })
    expect(h.bodies[1].context).toEqual({ text: 'Board: K2 Boards', origin: { widget: 'board-builder', garden: 'g-1', placement: 'board' } })
    expect(h.bodies[0].text, 'never folded into the body').toBe('ship it')
  })

  it('reads the daemon’s contextRefused (the message was still sent)', async () => {
    h.answer = { ...h.answer, contextRefused: { code: 'widget_not_approved', message: 'stale frame' } }
    const r = await postThreadCompose(scope, 'sales', 'ship it', null, {
      context: { text: 'Board: K2 Boards', origin: { widget: 'b', garden: 'g', placement: 'p', hash: 'h' } },
    })
    expect(r).toMatchObject({ ok: true, contextRefused: { code: 'widget_not_approved', message: 'stale frame' } })
    expect(r.ok && r.item?.doc.body).toBe('ship it')
  })
})

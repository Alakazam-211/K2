// App-governed Spaces helpers (prd-app-governed-spaces-k2-v1 §5.4, S8):
// the op builders check what K2 checks before anything is sent, and the
// two senders split long lists into calls of the allowed size, in order,
// and join the answers. A refused call throws and stops.

import { describe, expect, it } from 'vitest'

import {
  applyChannelOps,
  channelOp,
  setAllChannels,
  CHANNEL_OPS_PER_CALL,
  K2Error,
  MEMBER_CHANNELS_PER_CALL,
  type AppChatChannelOp,
  type AppChatMemberChannels,
  type ChatChannelsActor,
} from '../src/index.ts'

function fakeApp(fail?: { call: number; error: K2Error }) {
  const calls: { verb: string; args: { ops?: AppChatChannelOp[]; items?: AppChatMemberChannels[]; space?: string } }[] = []
  const app: ChatChannelsActor = {
    chat: {
      async channels(args) {
        calls.push({ verb: 'chat.channels', args })
        if (fail && calls.length === fail.call) throw fail.error
        return {
          ok: true,
          app: 'svc-1',
          spaceId: args.space ?? '5101',
          results: args.ops.map((o) => ({ op: o.op, slug: o.slug ?? null, channel_id: `id-${o.slug}`, name: o.name ?? null, created: true })),
        }
      },
      async setChannels(args) {
        calls.push({ verb: 'chat.setChannels', args })
        if (fail && calls.length === fail.call) throw fail.error
        return {
          ok: true,
          app: 'svc-1',
          spaceId: args.space ?? '5101',
          changed: args.items.length,
          items: args.items.map((i) => ({ username: i.username, ok: i.username !== 'ghost', ...(i.username === 'ghost' ? { error: 'not_found' } : {}) })),
        }
      },
    },
  }
  return { app, calls }
}

describe('channelOp', () => {
  it('builds the ops K2 sends to Sew, snake_case on the wire', () => {
    expect(channelOp.ensure('job-1042', ' Job 1042 ', { private: true })).toEqual({ op: 'ensure', slug: 'job-1042', name: 'Job 1042', type: 'text', private: true })
    expect(channelOp.ensure('site-call', 'Site call', { type: 'voice', userLimit: 12 })).toEqual({
      op: 'ensure',
      slug: 'site-call',
      name: 'Site call',
      type: 'voice',
      user_limit: 12,
    })
    expect(channelOp.rename('job-1042', 'Job 1042 (done)')).toEqual({ op: 'rename', slug: 'job-1042', name: 'Job 1042 (done)' })
    expect(channelOp.archive({ channelId: '48831765147549712' })).toEqual({ op: 'archive', channel_id: '48831765147549712' })
    expect(channelOp.unarchive('job-0917')).toEqual({ op: 'unarchive', slug: 'job-0917' })
  })

  it('refuses what K2 would refuse, as a usage K2Error', () => {
    const bad: [() => unknown, RegExp][] = [
      [() => channelOp.ensure('Job 1', 'x'), /isn't a channel slug/],
      [() => channelOp.ensure('-job', 'x'), /isn't a channel slug/],
      [() => channelOp.ensure('a'.repeat(81), 'x'), /isn't a channel slug/],
      [() => channelOp.ensure('job', '   '), /1 to 100 characters/],
      [() => channelOp.ensure('job', 'x'.repeat(101)), /1 to 100 characters/],
      [() => channelOp.ensure('job', 'x', { userLimit: 5 }), /userLimit is for voice/],
      [() => channelOp.ensure('job', 'x', { type: 'voice', userLimit: 100 }), /0 to 99/],
      [() => channelOp.ensure('job', 'x', { type: 'video' as 'voice' }), /'text' or 'voice'/],
      [() => channelOp.archive({ channelId: 'abc' }), /channelId/],
    ]
    for (const [f, re] of bad) {
      let err: unknown
      try {
        f()
      } catch (e) {
        err = e
      }
      expect(err).toBeInstanceOf(K2Error)
      expect((err as K2Error).code).toBe('usage')
      expect((err as K2Error).message + (err as K2Error).hint).toMatch(re)
    }
  })
})

describe('applyChannelOps', () => {
  it('sends 200 ops a call, in order, and joins the results', async () => {
    const { app, calls } = fakeApp()
    const ops = Array.from({ length: CHANNEL_OPS_PER_CALL * 2 + 1 }, (_, i) => channelOp.ensure(`job-${i}`, `Job ${i}`))
    const r = await applyChannelOps(app, ops, { space: '7001' })
    expect(calls.map((c) => c.args.ops?.length)).toEqual([200, 200, 1])
    expect(calls.every((c) => c.args.space === '7001')).toBe(true)
    expect(r.results).toHaveLength(401)
    expect(r.results[0]?.slug).toBe('job-0')
    expect(r.results[400]?.slug).toBe('job-400')
    expect(r.spaceId).toBe('7001')
  })

  it('leaves space out when not given, and refuses an empty list', async () => {
    const { app, calls } = fakeApp()
    await applyChannelOps(app, [channelOp.archive('job-1')])
    expect('space' in (calls[0]?.args ?? { space: 1 })).toBe(false)
    await expect(applyChannelOps(app, [])).rejects.toMatchObject({ code: 'usage' })
  })

  it('stops at the first refused call and throws it', async () => {
    const limited = new K2Error({ code: 'rate_limited', hint: 'at most 60 chat channel calls a minute per App', verb: 'chat.channels', status: 429, retryAfterSecs: 20 })
    const { app, calls } = fakeApp({ call: 2, error: limited })
    const ops = Array.from({ length: 450 }, (_, i) => channelOp.ensure(`job-${i}`, `Job ${i}`))
    await expect(applyChannelOps(app, ops)).rejects.toBe(limited)
    expect(calls).toHaveLength(2)
  })
})

describe('setAllChannels', () => {
  it('sends 500 people a call, adds up changed and keeps per-person refusals', async () => {
    const { app, calls } = fakeApp()
    const items = Array.from({ length: MEMBER_CHANNELS_PER_CALL + 3 }, (_, i) => ({ username: i === 2 ? 'ghost' : `user${i}`, channels: [`job-${i}`] }))
    const r = await setAllChannels(app, items)
    expect(calls.map((c) => [c.verb, c.args.items?.length])).toEqual([
      ['chat.setChannels', 500],
      ['chat.setChannels', 3],
    ])
    expect(r.changed).toBe(503)
    expect(r.items).toHaveLength(503)
    expect(r.items[2]).toEqual({ username: 'ghost', ok: false, error: 'not_found' })
  })

  it('refuses a person listed twice, a bad slug and an over-long list before sending', async () => {
    const { app, calls } = fakeApp()
    await expect(setAllChannels(app, [{ username: 'dana', channels: [] }, { username: 'Dana', channels: ['a'] }])).rejects.toMatchObject({ code: 'usage' })
    await expect(setAllChannels(app, [{ username: 'dana', channels: ['Not A Slug'] }])).rejects.toMatchObject({ code: 'usage' })
    await expect(setAllChannels(app, [{ username: 'dana', channels: Array.from({ length: 5001 }, (_, i) => `c${i}`) }])).rejects.toMatchObject({ code: 'usage' })
    await expect(setAllChannels(app, [])).rejects.toMatchObject({ code: 'usage' })
    expect(calls).toHaveLength(0)
  })
})

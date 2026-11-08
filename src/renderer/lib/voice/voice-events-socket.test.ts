// prd-voice-chat-only-members-v1 §6.3: `WS /cli/voice/events` for a server
// whose login never gets the app socket.

import { describe, expect, it } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'
import {
  subscribeVoiceEvents,
  voiceRoomsRev,
  VOICE_EVENTS_MAX_DEAD_TRIES,
  type VoiceEventsDeps,
} from './voice-events-socket'

class FakeSocket {
  url: string
  closed = false
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onclose: ((ev: { code: number }) => void) | null = null
  constructor(url: string) {
    this.url = url
  }
  close(): void {
    this.closed = true
  }
}

function harness(): { deps: VoiceEventsDeps; sockets: FakeSocket[]; fire: () => void; pending: () => number } {
  const sockets: FakeSocket[] = []
  let timers: Array<() => void> = []
  return {
    sockets,
    deps: {
      open: (url) => {
        const s = new FakeSocket(url)
        sockets.push(s)
        return s as unknown as WebSocket
      },
      setTimer: (fn) => {
        timers.push(fn)
        return timers.length
      },
      clearTimer: () => {
        timers = []
      },
    },
    fire: () => {
      const run = timers
      timers = []
      for (const t of run) t()
    },
    pending: () => timers.length,
  }
}

const scope = {
  hostKey: 'review.k2.dev',
  wsBase: async () => 'wss://review.k2.dev',
  creds: async () => ({ token: 'tok-r' }),
} as unknown as ServerScope

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve()
}

describe('voice events socket', () => {
  it('reads only voice_rooms_changed frames', () => {
    expect(voiceRoomsRev('{"kind":"voice_rooms_changed","rev":7}')).toBe(7)
    expect(voiceRoomsRev('{"kind":"voice_rooms_changed"}')).toBeNull()
    expect(voiceRoomsRev('{"kind":"ping"}')).toBeUndefined()
    expect(voiceRoomsRev('nope')).toBeUndefined()
    expect(voiceRoomsRev(42)).toBeUndefined()
  })

  it('opens /cli/voice/events with the login, hands revs on, and closes on unsubscribe', async () => {
    const hh = harness()
    const revs: Array<number | null> = []
    const stop = subscribeVoiceEvents(scope, (r) => revs.push(r), hh.deps)
    await settle()
    expect(hh.sockets).toHaveLength(1)
    expect(hh.sockets[0].url).toBe('wss://review.k2.dev/cli/voice/events?token=tok-r')
    hh.sockets[0].onopen?.()
    hh.sockets[0].onmessage?.({ data: '{"kind":"voice_rooms_changed","rev":3}' })
    hh.sockets[0].onmessage?.({ data: '{"kind":"ping"}' })
    expect(revs).toEqual([3])
    stop()
    expect(hh.sockets[0].closed).toBe(true)
    expect(hh.pending()).toBe(0)
  })

  it('reconnects after a drop, and gives up after failures that never opened (the sweep covers it)', async () => {
    const hh = harness()
    subscribeVoiceEvents(scope, () => {}, hh.deps)
    await settle()
    hh.sockets[0].onopen?.()
    hh.sockets[0].onclose?.({ code: 1006 })
    expect(hh.pending()).toBe(1)
    hh.fire()
    await settle()
    expect(hh.sockets).toHaveLength(2)
    // From here every try fails before it opens (a 404: the preview is off).
    for (let i = 0; i < VOICE_EVENTS_MAX_DEAD_TRIES; i++) {
      hh.sockets[hh.sockets.length - 1].onclose?.({ code: 1006 })
      hh.fire()
      await settle()
    }
    const opened = hh.sockets.length
    expect(opened).toBe(1 + VOICE_EVENTS_MAX_DEAD_TRIES)
    expect(hh.pending()).toBe(0)
  })
})

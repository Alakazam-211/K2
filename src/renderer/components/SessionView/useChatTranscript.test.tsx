// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'

const getDaemonWs = vi.hoisted(() => vi.fn(async () => ({
  host: '127.0.0.1',
  port: 9,
  token: 'tok',
  secure: false,
})))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs,
  daemonWsBase: () => 'ws://127.0.0.1:9',
}))

class FakeWS {
  static instances: FakeWS[] = []
  url: string
  closed = false
  onmessage: ((ev: { data: string }) => void) | null = null
  onerror: (() => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }
  close() {
    this.closed = true
  }
}

vi.stubGlobal('WebSocket', FakeWS)

import { useChatTranscript } from './useChatTranscript'

describe('useChatTranscript', () => {
  beforeEach(() => {
    FakeWS.instances = []
    getDaemonWs.mockClear()
  })

  it('does not open a socket without a provider conversation id', async () => {
    renderHook(() =>
      useChatTranscript({
        view: 'chat',
        visible: true,
        provider: 'codex',
        conversationId: null,
        agentName: 'tab-1',
      }),
    )
    await Promise.resolve()
    expect(getDaemonWs).not.toHaveBeenCalled()
    expect(FakeWS.instances).toHaveLength(0)
  })

  it('does not open a socket for thread, and closes when chat is no longer visible', async () => {
    const { rerender, unmount } = renderHook(
      (props: { view: 'chat' | 'thread'; visible: boolean }) =>
        useChatTranscript({
          view: props.view,
          visible: props.visible,
          provider: 'claude',
          conversationId: 'conv-1',
          agentName: 'tab-1',
        }),
      { initialProps: { view: 'thread' as 'chat' | 'thread', visible: true } },
    )
    await Promise.resolve()
    expect(FakeWS.instances).toHaveLength(0)

    rerender({ view: 'chat', visible: true })
    await waitFor(() => expect(FakeWS.instances).toHaveLength(1))
    expect(FakeWS.instances[0].url.includes('/cli/chat/transcript?')).toBe(true)
    expect(FakeWS.instances[0].url.includes('/cli/overlay/events')).toBe(false)

    rerender({ view: 'chat', visible: false })
    await waitFor(() => expect(FakeWS.instances[0].closed).toBe(true))
    unmount()
  })
})

// @vitest-environment jsdom
// prd-voice-chat-only-members-v1 §8.3, through the REAL ConnectionGate: a
// window whose server says this login is `voice_chat` never mounts the app.
// It shows the gate screen; an upgrade back to Member (whoami says so on the
// screen's re-check) mounts the app with no sign-in.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => {
  // Stores evaluated on import fetch at once; nothing here reaches a network.
  globalThis.fetch = (async () => new Response('{}', { status: 200 })) as typeof fetch
  return {
    app: { mounts: 0 },
    role: 'voice_chat',
    whoamis: 0,
    logins: 0,
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: async () => '9.9.9' }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))
vi.mock('@/components/TopBar/GateChrome', () => ({ default: () => null }))
vi.mock('@/App', async () => {
  const React = await import('react')
  function App(): React.JSX.Element {
    React.useEffect(() => {
      h.app.mounts += 1
    }, [])
    return <div data-stub-app="" />
  }
  return { default: App, HomeRoomsHost: () => null, ZenHost: () => null }
})

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { ConnectionGate } from './ConnectionGate'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useVoiceOpenRequest } from '@/stores/voice-open-request'
import { hostPool } from '@/lib/host-pool-instance'
import { VOICE_CHAT_GATE_RECHECK_MS, VoiceChatOnlyGate } from './VoiceChatOnlyGate'

const R: ConnectHost = {
  id: 'id-r', label: 'Review', hostname: 'review.k2.dev', port: 443, secure: true,
  username: 'reviewer', token: 'tok-r', remember: false, lastConnectedAt: null,
} as ConnectHost

const READY = JSON.stringify({ version: '9.9.9', protocol: 1, phase: 'ready', detail: '' })

beforeEach(() => {
  h.app.mounts = 0
  h.role = 'voice_chat'
  h.whoamis = 0
  h.logins = 0
  sessionStorage.clear()
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
  vi.stubGlobal('fetch', async (input: RequestInfo | URL) => {
    const url = String(input)
    if (url.includes('/boot-status')) return new Response(READY, { status: 200 })
    if (url.includes('/cli/auth/whoami')) {
      h.whoamis += 1
      return new Response(
        JSON.stringify({ username: 'reviewer', owner: false, role: h.role, voiceChatOnly: h.role === 'voice_chat', mustChangePassword: false }),
        { status: 200 },
      )
    }
    if (url.includes('/cli/auth/login')) {
      h.logins += 1
      return new Response('{}', { status: 500 })
    }
    return new Response(JSON.stringify({ error: 'role_required', required: 'member', role: h.role }), { status: 403 })
  })
  useConnectHostStore.setState({
    hosts: [R],
    activeHost: R,
    recovery: { kind: 'connected' },
    hydrateFromDisk: async () => undefined,
  } as never)
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
  useConnectHostStore.setState({ activeHost: 'local', hosts: [], recovery: { kind: 'connected' } } as never)
  useVoiceOpenRequest.setState({ pending: false })
})

describe('the Voice & Chat only gate screen', () => {
  it('a voice_chat whoami shows the screen, sets the hint, and never mounts the app', async () => {
    render(<ConnectionGate />)
    await waitFor(() => expect(screen.getByTestId('voice-chat-only-gate')).toBeTruthy(), { timeout: 5_000 })
    expect(screen.getByTestId('voice-chat-only-gate-text').textContent).toBe(
      "Review gives you Voice & Chat only. Join its rooms from the headset. You can't open this server or talk to its agents.",
    )
    expect(useConnectHostStore.getState().hosts[0].access).toBe('voice_chat')
    // Let the gate run a few more ticks: still no app, no sign-in.
    await new Promise((r) => setTimeout(r, 100))
    expect(h.app.mounts).toBe(0)
    expect(h.logins).toBe(0)
    expect(screen.getByTestId('voice-chat-only-open-rooms')).toBeTruthy()
    expect(screen.getByTestId('voice-chat-only-switch')).toBeTruthy()
    expect(screen.queryByTestId('voice-chat-only-sign-out')).toBeNull()
  })

  it('an old hint alone holds the screen before any check', () => {
    useConnectHostStore.setState({ hosts: [{ ...R, access: 'voice_chat' }], activeHost: { ...R, access: 'voice_chat' } } as never)
    render(<ConnectionGate />)
    expect(screen.getByTestId('voice-chat-only-gate')).toBeTruthy()
    expect(h.app.mounts).toBe(0)
  })

  it('Open voice rooms asks the headset to open and switches to This computer', async () => {
    useConnectHostStore.setState({ hosts: [{ ...R, access: 'voice_chat' }], activeHost: { ...R, access: 'voice_chat' } } as never)
    const pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost } as never)
    render(<ConnectionGate />)
    fireEvent.click(screen.getByTestId('voice-chat-only-open-rooms'))
    expect(useVoiceOpenRequest.getState().pending).toBe(true)
    expect(pickHost).toHaveBeenCalledWith('local')
  })

  it('an upgrade back to Member mounts the app on the next re-check, with no sign-in', async () => {
    render(<ConnectionGate />)
    await waitFor(() => expect(screen.getByTestId('voice-chat-only-gate')).toBeTruthy(), { timeout: 5_000 })
    h.role = 'member'
    // What the screen's timer runs every VOICE_CHAT_GATE_RECHECK_MS (the
    // timer itself is pinned below).
    await act(async () => {
      expect(await hostPool.refreshRole('review.k2.dev')).toBe('member')
    })
    await waitFor(() => expect(h.app.mounts).toBe(1), { timeout: 5_000 })
    expect(useConnectHostStore.getState().hosts[0].access).toBeUndefined()
    expect(screen.queryByTestId('voice-chat-only-gate')).toBeNull()
    expect(h.logins).toBe(0)
  })
})

describe('the gate screen on the web client', () => {
  it("can't carry a call: says so and offers only Sign out", async () => {
    const onSignOut = vi.fn(async () => undefined)
    const recheck = vi.fn()
    render(
      <VoiceChatOnlyGate
        server="Review"
        web
        onOpenVoiceRooms={() => {
          throw new Error('no voice rooms on the web client')
        }}
        onSwitchToThisComputer={() => {
          throw new Error('no This computer on the web client')
        }}
        onSignOut={onSignOut}
        recheck={recheck}
      />,
    )
    expect(screen.getByTestId('voice-chat-only-gate-text').textContent).toContain('Use the K2 desktop app to join its voice rooms.')
    expect(screen.queryByTestId('voice-chat-only-open-rooms')).toBeNull()
    expect(screen.queryByTestId('voice-chat-only-switch')).toBeNull()
    fireEvent.click(screen.getByTestId('voice-chat-only-sign-out'))
    await waitFor(() => expect(onSignOut).toHaveBeenCalledTimes(1))
    expect(recheck).toHaveBeenCalledTimes(1)
  })

  it('re-reads the role on mount and every VOICE_CHAT_GATE_RECHECK_MS, not on every render', () => {
    vi.useFakeTimers()
    const recheck = vi.fn()
    const props = {
      server: 'Review',
      web: false,
      onOpenVoiceRooms: () => {},
      onSwitchToThisComputer: () => {},
      onSignOut: () => {},
    }
    const { rerender } = render(<VoiceChatOnlyGate {...props} recheck={recheck} />)
    expect(recheck).toHaveBeenCalledTimes(1)
    // A health-poll re-render with a fresh callback neither re-sends nor resets.
    const next = vi.fn()
    rerender(<VoiceChatOnlyGate {...props} recheck={next} />)
    expect(recheck).toHaveBeenCalledTimes(1)
    expect(next).toHaveBeenCalledTimes(0)
    vi.advanceTimersByTime(VOICE_CHAT_GATE_RECHECK_MS)
    expect(next).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(VOICE_CHAT_GATE_RECHECK_MS)
    expect(next).toHaveBeenCalledTimes(2)
  })
})

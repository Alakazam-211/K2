// @vitest-environment jsdom
//
// Voice rooms lane D (§4.1–§4.6): the headset hides unless a signed-in
// server reports `voice-rooms-v1` (or shows dimmed, with the window server's
// reason, while that server has the preview on but doesn't host); the list shows "This server" first, the
// call on top, join / leave against the fake engine, the confirm when
// switching calls, the signed-out row and the lane-reconnecting line.

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useStore } from 'zustand'
import type { HostEntry } from '@/lib/host-pool'
import type { ServerScope } from '@/kessel/server-scope'
import type { VoiceRoomsList } from '@/lib/voice/voice-wire'

const h = vi.hoisted(() => ({
  entries: {} as Record<string, unknown>,
  lists: {} as Record<string, unknown>,
  signIn: vi.fn(),
  preview: false,
  setSection: vi.fn(),
}))

vi.mock('@/stores/settings', () => {
  const state = (): { voiceRoomsPreview: boolean; setSection: (s: string) => void } => ({ voiceRoomsPreview: h.preview, setSection: h.setSection })
  const useSettingsStore = <T,>(sel: (s: ReturnType<typeof state>) => T): T => sel(state())
  useSettingsStore.getState = state
  return { useSettingsStore }
})

vi.mock('@/components/Home/room/RoomFailure', () => ({
  roomFailureActions: (hostKey: string) => ({ signIn: () => h.signIn(hostKey), retry: () => {}, openServer: () => {} }),
}))

vi.mock('@/stores/voice', async () => {
  const core = await import('@/stores/voice-core')
  const { FakeVoiceEngine } = await import('@/lib/voice/fake-voice-engine')
  const voice = core.createVoiceController({
    keys: () => ['zed.k2.dev', 'local', 'sewlab.k2.dev', 'alpha.k2.dev'],
    windowKey: () => 'zed.k2.dev',
    labelOf: (k: string) => ({ 'zed.k2.dev': 'zed', 'sewlab.k2.dev': 'sewlab', 'alpha.k2.dev': 'Alpha', local: 'This computer' })[k] ?? k,
    entries: () => h.entries as Record<string, HostEntry>,
    check: async () => undefined,
    onPoolChange: () => () => {},
    onAuth: () => () => {},
    onWindowRoomsChanged: () => () => {},
    onRoomsChanged: () => () => {},
    selfNames: () => [],
    scopeFor: (k: string) => ({ hostKey: k, id: k, label: k }) as unknown as ServerScope,
    api: {
      rooms: async (s: ServerScope) => {
        const l = h.lists[s.hostKey]
        if (!l) throw new Error('no list')
        return l as VoiceRoomsList
      },
      join: async (_s: ServerScope, roomId: string) => ({
        callId: `call-${roomId}`,
        roomId,
        roomName: roomId === 'room-1' ? 'Standup' : 'Room 2',
        signalUrl: 'wss://voice.sewlab.k2.dev',
        mediaToken: 't',
        e2ee: null,
        relayOnly: true,
      }),
      leave: async () => {},
      selfState: async () => {},
    },
    makeEngine: () => new FakeVoiceEngine(),
    toast: () => {},
    loadReported: () => new Set(['alpha.k2.dev']),
    saveReported: () => {},
    loadPrefs: () => ({ inputDeviceId: null, outputDeviceId: null, joinMuted: false }),
    savePrefs: () => {},
    now: () => Date.now(),
  })
  return {
    voice,
    retainVoice: () => () => {},
    useVoice: <T,>(sel: (s: ReturnType<typeof voice.store.getState>) => T): T => useStore(voice.store, sel),
  }
})

import VoiceButton from './VoiceButton'
import { voice } from '@/stores/voice'
import { overlayClaims } from '@/lib/browser-overlay'

function entry(hostKey: string, patch: Partial<HostEntry> = {}, features = ['voice-rooms-v1']): HostEntry {
  return {
    hostKey,
    saved: true,
    hostId: hostKey,
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.46.8', protocol: 1, instanceId: 'i', features, view: 'full', at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: 1,
    ...patch,
  }
}

const sewList: VoiceRoomsList = {
  hosting: { state: 'ok', line: 'Voice rooms are on.', detail: null },
  canManage: false,
  feedLive: true,
  rev: 1,
  rooms: [
    { id: 'room-1', name: 'Standup', limit: 25, closing: false, closesAt: null, unavailable: null, occupants: [{ user: 'ada', name: 'Ada', role: 'admin', color: null, muted: false, deafened: false, speaking: false }, { user: 'bo', name: null, role: null, color: null, muted: false, deafened: false, speaking: false }] },
    { id: 'room-2', name: 'Room 2', limit: 2, closing: false, closesAt: null, unavailable: null, occupants: [] },
  ],
}

beforeEach(async () => {
  h.entries = {}
  h.lists = {}
  h.signIn.mockReset()
  h.setSection.mockReset()
  h.preview = false
  await act(async () => {
    if (voice.store.getState().call) await voice.leave()
    voice.store.setState({ rooms: {}, notice: null, popoverOpen: false, windowHosting: null })
    voice.recompute()
  })
})
afterEach(cleanup)

describe('VoiceButton', () => {
  it('renders nothing while no signed-in server reports voice rooms (preview off)', () => {
    h.entries = { 'zed.k2.dev': entry('zed.k2.dev', {}, []), 'sewlab.k2.dev': entry('sewlab.k2.dev', {}, []) }
    act(() => voice.recompute())
    const { container } = render(<VoiceButton withPipe />)
    expect(container.innerHTML).toBe('')
  })

  it("preview on but this server doesn't host: a dimmed headset whose list says why, in the daemon's words", async () => {
    h.preview = true
    h.entries = { 'zed.k2.dev': entry('zed.k2.dev', { role: 'owner' }, []) }
    h.lists = {
      'zed.k2.dev': {
        hosting: { state: 'not_linux', line: 'Voice rooms are hosted on Linux servers. Sign in to one from the server list to join its rooms.', detail: null },
        canManage: true,
        rev: 1,
        rooms: [],
      },
    }
    act(() => voice.recompute())
    const { container } = render(<VoiceButton withPipe />)
    const button = screen.getByTestId('voice-button')
    expect(button.getAttribute('data-look')).toBe('unavailable')
    expect(button.className).toContain('opacity-40')
    expect(button.getAttribute('title')).toBe("Voice rooms: this server doesn't host them. Click to see why.")
    expect(container.querySelector('[data-top-bar-pipe]')).not.toBeNull()

    fireEvent.click(button)
    await waitFor(() => expect(screen.getByTestId('voice-window-hosting').getAttribute('data-state')).toBe('not_linux'))
    expect(screen.getByTestId('voice-window-hosting').textContent).toContain('This server · zed')
    expect(screen.getByTestId('voice-window-hosting-line').textContent).toBe(
      'Voice rooms are hosted on Linux servers. Sign in to one from the server list to join its rooms. Settings',
    )
    // No "No voice rooms yet" line under the reason.
    expect(screen.queryByTestId('voice-empty')).toBeNull()
    fireEvent.click(screen.getByTestId('voice-open-settings'))
    expect(h.setSection).toHaveBeenCalledWith('voice-rooms')
  })

  it("the dimmed list shows the reason's detail, no Settings link for a Member, and a failed read", async () => {
    h.preview = true
    h.entries = { 'zed.k2.dev': entry('zed.k2.dev', {}, []) }
    h.lists = {
      'zed.k2.dev': {
        hosting: { state: 'not_pro', line: 'Voice rooms need the Pro plan.', detail: 'plan: free' },
        canManage: false,
        rev: 1,
        rooms: [],
      },
    }
    act(() => voice.recompute())
    render(<VoiceButton />)
    fireEvent.click(screen.getByTestId('voice-button'))
    await waitFor(() => expect(screen.getByTestId('voice-window-hosting-line').textContent).toBe('Voice rooms need the Pro plan.'))
    expect(screen.getByTestId('voice-window-hosting-detail').textContent).toBe('plan: free')
    expect(screen.queryByTestId('voice-open-settings')).toBeNull()

    // Closed, then opened again with the route failing: the error, not a stale line.
    fireEvent.click(screen.getByTestId('voice-button'))
    h.lists = {}
    fireEvent.click(screen.getByTestId('voice-button'))
    await waitFor(() => expect(screen.getByTestId('voice-window-hosting-error').textContent).toContain('no list'))
    expect(screen.queryByTestId('voice-window-hosting-line')).toBeNull()
  })

  it('a server that hosts again turns the dimmed headset back into the normal one', () => {
    h.preview = true
    h.entries = { 'zed.k2.dev': entry('zed.k2.dev', {}, []) }
    act(() => voice.recompute())
    render(<VoiceButton />)
    expect(screen.getByTestId('voice-button').getAttribute('data-look')).toBe('unavailable')
    h.entries = { 'zed.k2.dev': entry('zed.k2.dev') }
    act(() => voice.recompute())
    expect(screen.getByTestId('voice-button').getAttribute('data-look')).toBe('rest')
  })

  it('shows the headset with its own divider when the roster is hidden', () => {
    h.entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev') }
    act(() => voice.recompute())
    const { container } = render(<VoiceButton withPipe />)
    expect(screen.getByTestId('voice-button')).toBeTruthy()
    expect(container.querySelector('[data-top-bar-pipe]')).not.toBeNull()
  })

  it('the open list claims its own box over the Browser page, and the claim goes with the list', () => {
    h.entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev') }
    act(() => voice.recompute())
    render(<VoiceButton />)
    expect(overlayClaims()).toEqual([])

    fireEvent.click(screen.getByTestId('voice-button'))
    const popover = screen.getByTestId('voice-popover')
    expect(overlayClaims().map((c) => [c.kind, c.element])).toEqual([['popover', popover]])

    fireEvent.click(screen.getByTestId('voice-button'))
    expect(overlayClaims()).toEqual([])

    // The headset hides with the list still flagged open (the last voice
    // server signed out): nothing may keep covering the page.
    fireEvent.click(screen.getByTestId('voice-button'))
    expect(overlayClaims()).toHaveLength(1)
    h.entries = {}
    act(() => voice.recompute())
    expect(screen.queryByTestId('voice-popover')).toBeNull()
    expect(voice.store.getState().popoverOpen).toBe(true)
    expect(overlayClaims()).toEqual([])
  })

  it('lists This server first, then A to Z; occupied tooltip; join, the call on top, then leave', async () => {
    h.entries = {
      'zed.k2.dev': entry('zed.k2.dev', { auth: 'signin-required' }),
      'sewlab.k2.dev': entry('sewlab.k2.dev'),
      'alpha.k2.dev': entry('alpha.k2.dev', { auth: 'signin-required' }),
    }
    h.lists = {
      'zed.k2.dev': { hosting: { state: 'voice_lane_reconnecting', line: "Voice is reconnecting to K2's network…", detail: null }, canManage: true, rev: 1, rooms: [] },
      'sewlab.k2.dev': sewList,
    }
    act(() => voice.recompute())
    render(<VoiceButton />)
    fireEvent.click(screen.getByTestId('voice-button'))
    await waitFor(() => expect(screen.getAllByTestId('voice-room')).toHaveLength(2))

    const sections = screen.getAllByTestId('voice-server')
    expect(sections.map((s) => s.getAttribute('data-kind'))).toEqual(['live', 'signed-out', 'live'])
    expect(sections[0].textContent).toContain('This server · zed')
    expect(sections[1].textContent).toContain('Alpha')
    expect(sections[2].textContent).toContain('sewlab')
    // §6.6 M11: under 20 s of lane-down the window's server says reconnecting.
    expect(screen.getByTestId('voice-reconnecting').textContent).toBe("Voice is reconnecting to K2's network…")
    expect(screen.getByTestId('voice-button').getAttribute('title')).toBe('Ada and 1 other are in Standup · sewlab')

    fireEvent.click(screen.getByTestId('voice-sign-in'))
    expect(h.signIn).toHaveBeenCalledWith('alpha.k2.dev')

    fireEvent.click(screen.getAllByTestId('voice-join')[0])
    await waitFor(() => expect(screen.getByTestId('voice-call').getAttribute('data-phase')).toBe('connected'))
    expect(screen.getByTestId('voice-relay-only')).toBeTruthy()
    // No audio in this version, and the popover says so.
    expect(screen.getByTestId('voice-seat-only').textContent).toContain('No call audio')
    expect(screen.getByTestId('voice-button').getAttribute('data-look')).toBe('in-call')

    fireEvent.click(screen.getByTestId('voice-mute'))
    expect(screen.getByTestId('voice-icon').getAttribute('data-muted')).toBe('true')

    fireEvent.click(screen.getByTestId('voice-leave'))
    await waitFor(() => expect(screen.queryByTestId('voice-call')).toBeNull())
    expect(screen.getByTestId('voice-notice').textContent).toBe('You left Standup.')
  })

  it('joining another room while in a call asks first', async () => {
    h.entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev') }
    h.lists = { 'sewlab.k2.dev': sewList }
    act(() => voice.recompute())
    render(<VoiceButton />)
    fireEvent.click(screen.getByTestId('voice-button'))
    await waitFor(() => expect(screen.getAllByTestId('voice-join')).toHaveLength(2))
    fireEvent.click(screen.getAllByTestId('voice-join')[0])
    await waitFor(() => expect(screen.getByTestId('voice-call').getAttribute('data-phase')).toBe('connected'))
    // Room 2 is the only Join left (Standup says "You're here").
    fireEvent.click(screen.getByTestId('voice-join'))
    expect(screen.getByTestId('voice-confirm').textContent).toContain('Leave Standup and join Room 2?')
    fireEvent.click(screen.getByTestId('voice-confirm-join'))
    await waitFor(() => expect(voice.store.getState().call?.roomId).toBe('room-2'))
  })

  it('a full, closing or unavailable room offers no Join', async () => {
    h.entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev') }
    h.lists = {
      'sewlab.k2.dev': {
        ...sewList,
        rooms: [
          // The daemon says `room_full`; it must read Full, not Unavailable.
          { ...sewList.rooms[1], occupants: sewList.rooms[0].occupants, unavailable: 'room_full' },
          { ...sewList.rooms[1], id: 'room-3', name: 'Room 3', closing: true },
          { ...sewList.rooms[1], id: 'room-4', name: 'Room 4', unavailable: 'media_path_down' },
          { ...sewList.rooms[1], id: 'room-5', name: 'Room 5', closing: true, closesAt: Date.now() + 8_000 },
        ],
      },
    }
    act(() => voice.recompute())
    render(<VoiceButton />)
    fireEvent.click(screen.getByTestId('voice-button'))
    await waitFor(() => expect(screen.getAllByTestId('voice-room-state')).toHaveLength(4))
    expect(screen.getAllByTestId('voice-room-state').map((e) => e.textContent)).toEqual(['Full', 'Closing', 'Unavailable', 'Closes in 8 s'])
    expect(screen.queryByTestId('voice-join')).toBeNull()
  })

  it('devices use K2 menus: System default first, a pick sets the pref and keeps the list open', async () => {
    h.entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev') }
    h.lists = { 'sewlab.k2.dev': sewList }
    act(() => voice.recompute())
    render(<VoiceButton />)
    fireEvent.click(screen.getByTestId('voice-button'))
    await waitFor(() => expect(screen.getAllByTestId('voice-join')).toHaveLength(2))
    fireEvent.click(screen.getAllByTestId('voice-join')[0])
    await waitFor(() => expect(screen.getByTestId('voice-input')).toBeTruthy())

    const input = screen.getByTestId('voice-input')
    expect(input.tagName).toBe('BUTTON')
    expect(document.querySelector('select')).toBeNull()
    expect(input.textContent).toBe('System default')

    fireEvent.click(input)
    const menu = screen.getByTestId('setting-dropdown-menu')
    // Portaled out of the popover, above it.
    expect(screen.getByTestId('voice-popover').contains(menu)).toBe(false)
    expect(Number(menu.style.zIndex)).toBeGreaterThan(50)
    expect(Array.from(menu.querySelectorAll('button')).map((b) => b.textContent)).toEqual(['System default', 'Fake microphone'])
    const pick = Array.from(menu.querySelectorAll('button')).find((b) => b.textContent === 'Fake microphone')
    if (!pick) throw new Error('no Fake microphone option')
    // A mousedown inside the portaled menu is not outside the popover.
    fireEvent.mouseDown(pick)
    fireEvent.click(pick)
    expect(voice.store.getState().prefs.inputDeviceId).toBe('fake-mic')
    expect(screen.getByTestId('voice-popover')).toBeTruthy()
    expect(screen.getByTestId('voice-input').textContent).toBe('Fake microphone')

    // Back to System default → null.
    fireEvent.click(screen.getByTestId('voice-input'))
    const back = Array.from(screen.getByTestId('setting-dropdown-menu').querySelectorAll('button')).find((b) => b.textContent === 'System default')
    if (!back) throw new Error('no System default option')
    fireEvent.mouseDown(back)
    fireEvent.click(back)
    expect(voice.store.getState().prefs.inputDeviceId).toBeNull()

    // Speakers: enabled while the engine can route output; disabled with the tooltip when it can't.
    const output = screen.getByTestId('voice-output') as HTMLButtonElement
    expect(output.disabled).toBe(false)
    act(() => {
      const d = voice.store.getState().devices
      if (!d) throw new Error('devices not loaded')
      voice.store.setState({ devices: { ...d, outputSelectable: false } })
    })
    const off = screen.getByTestId('voice-output') as HTMLButtonElement
    expect(off.disabled).toBe(true)
    expect(off.getAttribute('title')).toBe('This system plays calls on its default output.')
    fireEvent.click(off)
    expect(screen.queryByTestId('setting-dropdown-menu')).toBeNull()

    // A mousedown truly outside still closes the popover.
    fireEvent.mouseDown(document.body)
    expect(screen.queryByTestId('voice-popover')).toBeNull()
  })
})

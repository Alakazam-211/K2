// @vitest-environment jsdom
//
// Voice rooms lane D (§4.1–§4.6): the headset hides unless a signed-in
// server reports `voice-rooms-v1`; the list shows "This server" first, the
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
}))

vi.mock('@/stores/settings', () => ({ useSettingsStore: { getState: () => ({ setSection: () => {} }) } }))

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
  await act(async () => {
    if (voice.store.getState().call) await voice.leave()
    voice.store.setState({ rooms: {}, notice: null, popoverOpen: false })
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

  it('shows the headset with its own divider when the roster is hidden', () => {
    h.entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev') }
    act(() => voice.recompute())
    const { container } = render(<VoiceButton withPipe />)
    expect(screen.getByTestId('voice-button')).toBeTruthy()
    expect(container.querySelector('[data-top-bar-pipe]')).not.toBeNull()
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
})

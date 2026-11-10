// @vitest-environment jsdom
//
// Settings → Voice Rooms (prd-k2-voice-rooms-v1 §3.3, §3.4, §4.5): read-only
// for Members (the daemon's `canManage`, never a guessed role); Owner/Admin
// get the count stepper, "Close now" when lowering past occupied rooms, and
// renames; the hosting line and the media mode show.

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  rooms: vi.fn(),
  status: vi.fn(),
  setCount: vi.fn(),
  rename: vi.fn(),
  setRequired: vi.fn(),
  changed: [] as Array<() => void>,
}))

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => {}) }))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => ({ hostKey: 'sewlab.k2.dev', id: 'p' }) }))
vi.mock('@/stores/session-events', () => ({
  onVoiceRoomsChanged: (_scope: unknown, fn: () => void) => {
    h.changed.push(fn)
    return () => {
      h.changed.splice(h.changed.indexOf(fn), 1)
    }
  },
}))
vi.mock('@/stores/settings', () => ({ useSettingsStore: { getState: () => ({ setSection: () => {} }) } }))
vi.mock('@/stores/voice', () => ({
  useVoice: (sel: (s: { prefs: { joinMuted: boolean } }) => unknown) => sel({ prefs: { joinMuted: false } }),
  voice: { refreshRooms: async () => {}, setPrefs: () => {} },
}))
vi.mock('@/lib/voice/voice-api', () => ({
  fetchVoiceRooms: (...a: unknown[]) => h.rooms(...a),
  fetchVoiceStatus: (...a: unknown[]) => h.status(...a),
  setVoiceRoomCount: (...a: unknown[]) => h.setCount(...a),
  renameVoiceRoom: (...a: unknown[]) => h.rename(...a),
  setVoiceRecordingRequired: (...a: unknown[]) => h.setRequired(...a),
}))

import { VoiceRoomsSection } from './VoiceRoomsSection'

const occupant = { user: 'ada', name: null, role: null, color: null, muted: false, deafened: false, speaking: false }
const rooms = (canManage: boolean) => ({
  hosting: { state: 'ok', line: 'Voice rooms are on.', detail: null },
  canManage,
  feedLive: true,
  rev: 1,
  rooms: [
    { id: 'room-1', name: 'Room 1', limit: 25, closing: false, closesAt: null, unavailable: null, occupants: [] },
    { id: 'room-2', name: 'Room 2', limit: 25, closing: false, closesAt: null, unavailable: null, occupants: [occupant] },
  ],
})

beforeEach(() => {
  for (const f of [h.rooms, h.status, h.setCount, h.rename, h.setRequired]) f.mockReset()
  h.setRequired.mockResolvedValue(undefined)
  h.changed.length = 0
  h.setCount.mockResolvedValue(undefined)
  h.rename.mockResolvedValue(undefined)
})
afterEach(cleanup)

describe('VoiceRoomsSection', () => {
  it('a Member sees the rooms read-only and never asks for status', async () => {
    h.rooms.mockResolvedValue(rooms(false))
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getAllByTestId('voice-rooms-row')).toHaveLength(2))
    expect(screen.queryByTestId('voice-rooms-count')).toBeNull()
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(h.status).not.toHaveBeenCalled()
  })

  it('an Owner lowers the count past an occupied room with Close now', async () => {
    h.rooms.mockResolvedValue(rooms(true))
    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: 'tunnel' })
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getByTestId('voice-rooms-count').textContent).toBe('2'))
    expect(screen.getByTestId('voice-rooms-media').textContent).toContain("isn't published")
    fireEvent.click(screen.getByTestId('voice-rooms-minus'))
    fireEvent.click(screen.getByTestId('voice-rooms-close-now'))
    fireEvent.click(screen.getByTestId('voice-rooms-apply'))
    await waitFor(() => expect(h.setCount).toHaveBeenCalledWith({ hostKey: 'sewlab.k2.dev', id: 'p' }, 1, true))
  })

  it('renames a room', async () => {
    h.rooms.mockResolvedValue(rooms(true))
    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: null })
    render(<VoiceRoomsSection />)
    const input = await screen.findByLabelText('Name of Room 1')
    fireEvent.change(input, { target: { value: 'Standup' } })
    fireEvent.click(screen.getByText('Rename'))
    await waitFor(() => expect(h.rename).toHaveBeenCalledWith({ hostKey: 'sewlab.k2.dev', id: 'p' }, 'room-1', 'Standup'))
  })

  it('a Mac server with the preview on says why, and has no stepper or renames', async () => {
    const line = 'Voice rooms are hosted on Linux servers. Sign in to one from the server list to join its rooms.'
    h.rooms.mockResolvedValue({ ...rooms(true), hosting: { state: 'not_linux', line, detail: null }, rooms: [] })
    h.status.mockResolvedValue({ hosting: { state: 'not_linux', line, detail: null }, count: 1, mediaTier: null })
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getByTestId('voice-rooms-hosting').textContent).toContain('hosted on Linux servers'))
    expect(screen.queryByTestId('voice-rooms-count')).toBeNull()
  })

  it('re-reads on voice_rooms_changed, and stops listening when closed', async () => {
    h.rooms.mockResolvedValue(rooms(false))
    const { unmount } = render(<VoiceRoomsSection />)
    await waitFor(() => expect(h.rooms).toHaveBeenCalledTimes(1))
    expect(h.changed).toHaveLength(1)
    h.changed[0]()
    await waitFor(() => expect(h.rooms).toHaveBeenCalledTimes(2))
    unmount()
    expect(h.changed).toHaveLength(0)
  })

  it('"Require recording" stays hidden until K2 answers consent prompts (consentSheet), then the owner sets it', async () => {
    const recording = { spaceId: '4001', startLevel: null, required: null, locked: false, canRequire: true, consentSheet: false }
    h.rooms.mockResolvedValue(rooms(true))
    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: null, recording })
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getByTestId('voice-rooms-count').textContent).toBe('2'))
    expect(screen.queryByTestId('voice-rooms-require-recording')).toBeNull()
    expect(screen.queryByText(/Require recording/)).toBeNull()
    cleanup()

    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: null, recording: { ...recording, consentSheet: true } })
    render(<VoiceRoomsSection />)
    const sw = await screen.findByTestId('voice-rooms-require-recording')
    expect(screen.getByText(/Require recording when two or more people are in a room/)).toBeTruthy()
    fireEvent.click(sw)
    await waitFor(() => expect(h.setRequired).toHaveBeenCalledWith({ hostKey: 'sewlab.k2.dev', id: 'p' }, true))
  })

  it('shows why the server cannot host (§4.5)', async () => {
    h.rooms.mockResolvedValue({ ...rooms(true), hosting: { state: 'not_pro', line: 'Voice rooms need the Pro plan.', detail: null }, rooms: [] })
    h.status.mockResolvedValue({ hosting: { state: 'not_pro', line: 'Voice rooms need the Pro plan.', detail: null }, count: 1, mediaTier: null })
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getByTestId('voice-rooms-hosting').textContent).toContain('Voice rooms need the Pro plan.'))
  })
})

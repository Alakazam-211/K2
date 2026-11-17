// @vitest-environment jsdom
//
// Settings → Voice Rooms (prd-k2-voice-rooms-v1 §3.3, §3.4, §4.5): read-only
// for Members (the daemon's `canManage`, never a guessed role); Owner/Admin
// get the count stepper, "Close now" when lowering past occupied rooms, and
// renames; the hosting line and the media mode show; an odd or refused read
// is a plain error card with Try again, never a raw parser line.

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
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
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

import ConfirmDialog from '@/components/ConfirmDialog/ConfirmDialog'
import { DaemonHttpError } from '@/lib/daemon-http-error'
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
    render(
      <>
        <VoiceRoomsSection />
        <ConfirmDialog />
      </>,
    )
    await waitFor(() => expect(screen.getByTestId('voice-rooms-count').textContent).toBe('2'))
    expect(screen.getByTestId('voice-rooms-media').textContent).toContain('isn’t published')
    fireEvent.click(screen.getByTestId('voice-rooms-minus'))
    fireEvent.click(screen.getByTestId('voice-rooms-close-now'))
    fireEvent.click(screen.getByTestId('voice-rooms-apply'))
    // Cutting people off asks first.
    await waitFor(() => expect(screen.getByText('Close this room now?')).toBeTruthy())
    expect(h.setCount).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Close now' }))
    await waitFor(() => expect(h.setCount).toHaveBeenCalledWith({ hostKey: 'sewlab.k2.dev', id: 'p' }, 1, true))
    await waitFor(() => expect(screen.getByTestId('voice-rooms-count-note').textContent).toContain('Saved.'))
  })

  it('lowering without Close now asks nothing and lets rooms close once empty', async () => {
    h.rooms.mockResolvedValue(rooms(true))
    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: 'tunnel' })
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getByTestId('voice-rooms-count').textContent).toBe('2'))
    fireEvent.click(screen.getByTestId('voice-rooms-minus'))
    fireEvent.click(screen.getByTestId('voice-rooms-apply'))
    await waitFor(() => expect(h.setCount).toHaveBeenCalledWith({ hostKey: 'sewlab.k2.dev', id: 'p' }, 1, false))
  })

  it('a refused count change says what failed and the server’s reason', async () => {
    h.rooms.mockResolvedValue(rooms(true))
    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: null })
    h.setCount.mockRejectedValue(new DaemonHttpError('voice settings are the owner’s and Admins’', 403, 'owner_or_admin_only'))
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getByTestId('voice-rooms-count').textContent).toBe('2'))
    fireEvent.click(screen.getByTestId('voice-rooms-plus'))
    fireEvent.click(screen.getByTestId('voice-rooms-apply'))
    const note = await screen.findByTestId('voice-rooms-count-note')
    expect(note.getAttribute('role')).toBe('alert')
    expect(note.textContent).toBe('Couldn’t change the number of rooms. Nothing changed; try again.The server said: voice settings are the owner’s and Admins’.')
  })

  it('an odd rooms reply is a plain error card with Try again, never a raw parser line', async () => {
    h.rooms.mockRejectedValueOnce(new Error("the server's answer was missing the list of rooms"))
    h.rooms.mockResolvedValue(rooms(false))
    render(<VoiceRoomsSection />)
    const card = await screen.findByTestId('voice-rooms-error')
    expect(card.getAttribute('role')).toBe('alert')
    expect(card.textContent).toContain('Couldn’t load the voice rooms')
    expect(card.textContent).toContain("The server's answer was missing the list of rooms.")
    expect(card.textContent).not.toContain('voice/rooms')
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getAllByTestId('voice-rooms-row')).toHaveLength(2))
    expect(screen.queryByTestId('voice-rooms-error')).toBeNull()
  })

  it('renames a room', async () => {
    h.rooms.mockResolvedValue(rooms(true))
    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: null })
    render(<VoiceRoomsSection />)
    const input = await screen.findByLabelText('Name of Room 1')
    fireEvent.change(input, { target: { value: 'Standup' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Rename' })[0])
    await waitFor(() => expect(h.rename).toHaveBeenCalledWith({ hostKey: 'sewlab.k2.dev', id: 'p' }, 'room-1', 'Standup'))
    await waitFor(() => expect(screen.getByTestId('voice-rooms-note').textContent).toBe('Renamed “Room 1” to “Standup”.'))
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
    expect(screen.queryByTestId('voice-rooms-agent-readers')).toBeNull()
  })

  it("the room's recording settings say when agents may read the transcript (0.46.11)", async () => {
    const recording = { spaceId: '4001', startLevel: null, required: null, locked: false, canRequire: true, consentSheet: true, agentReaders: true }
    h.rooms.mockResolvedValue(rooms(true))
    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: null, recording })
    render(<VoiceRoomsSection />)
    const line = await screen.findByTestId('voice-rooms-agent-readers')
    expect(line.textContent).toContain('Agents on this server may read the transcript.')
  })

  it('shows why the server cannot host (§4.5)', async () => {
    h.rooms.mockResolvedValue({ ...rooms(true), hosting: { state: 'not_pro', line: 'Voice rooms need the Pro plan.', detail: null }, rooms: [] })
    h.status.mockResolvedValue({ hosting: { state: 'not_pro', line: 'Voice rooms need the Pro plan.', detail: null }, count: 1, mediaTier: null })
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getByTestId('voice-rooms-hosting').textContent).toContain('Voice rooms need the Pro plan.'))
    expect(screen.getByRole('button', { name: 'See plans' })).toBeTruthy()
  })

  it('says a hosting state in plain words, with the daemon’s detail quietly under it', async () => {
    const hosting = { state: 'sew_off', line: 'Voice rooms are off on this server: Chat / Voice Hosting isn’t on (k2 hostchat status).', detail: 'identity: standalone' }
    h.rooms.mockResolvedValue({ ...rooms(false), hosting, rooms: [] })
    render(<VoiceRoomsSection />)
    const box = await screen.findByTestId('voice-rooms-hosting')
    expect(box.textContent).toContain('Voice rooms need chat and voice')
    expect(box.textContent).not.toContain('k2 hostchat')
    expect(box.textContent).toContain('The server said: identity: standalone.')
  })

  it('a server with no rooms says so once, with the next step', async () => {
    const hosting = { state: 'count_zero', line: 'This server hosts no rooms.', detail: null }
    h.rooms.mockResolvedValue({ ...rooms(true), hosting, rooms: [] })
    h.status.mockResolvedValue({ hosting, count: 0, mediaTier: null })
    render(<VoiceRoomsSection />)
    const empty = await screen.findByTestId('voice-rooms-empty')
    expect(empty.textContent).toContain('Raise the number of rooms above to add one.')
    expect(screen.queryByTestId('voice-rooms-hosting')).toBeNull()
  })

  it('Save says Saving… and Rename says Renaming… while the write is in flight', async () => {
    h.rooms.mockResolvedValue(rooms(true))
    h.status.mockResolvedValue({ hosting: { state: 'ok', line: null, detail: null }, count: 2, mediaTier: null })
    let release: () => void = () => {}
    h.setCount.mockReturnValue(new Promise<void>((r) => (release = r)))
    h.rename.mockReturnValue(new Promise<void>(() => {}))
    render(<VoiceRoomsSection />)
    await waitFor(() => expect(screen.getByTestId('voice-rooms-count').textContent).toBe('2'))
    fireEvent.click(screen.getByTestId('voice-rooms-plus'))
    fireEvent.click(screen.getByTestId('voice-rooms-apply'))
    await waitFor(() => expect(screen.getByTestId('voice-rooms-apply').textContent).toBe('Saving…'))
    release()
    await waitFor(() => expect(screen.getByTestId('voice-rooms-apply').textContent).toBe('Save'))
    fireEvent.change(screen.getByLabelText('Name of Room 1'), { target: { value: 'Standup' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Rename' })[0])
    await waitFor(() => expect(screen.getByRole('button', { name: 'Renaming…' })).toBeTruthy())
  })
})

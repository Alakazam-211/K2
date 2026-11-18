// @vitest-environment jsdom
// A voice room's recordings drawer: loading, empty, error and the list
// (newest first, the DM's recording card), Show transcript only where this
// login may read, and the DM's own transcript panel fed page by page, with
// a refusal said in plain words. One list read per open.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('@/lib/daemon-cli', async (orig) => ({
  ...(await orig<typeof import('@/lib/daemon-cli')>()),
  daemonCliGet: (...a: unknown[]) => h.get(...a),
}))

import { DaemonHttpError } from '@/lib/daemon-http-error'
import { NO_RECORDINGS_LINE, RecordingsDrawer, SIGN_IN_TO_READ_LINE, VoiceRoomRecordingsSheet, useRoomRecordingsSheet, type RoomRecordingsTarget } from './VoiceRoomRecordings'

const target: RoomRecordingsTarget = {
  hostKey: 'zed.k2.dev',
  roomId: 'room-1',
  roomName: 'Standup',
  occupants: [{ user: 'ada', name: 'Ada Lovelace', memberId: '7', role: 'admin', color: null, muted: false, deafened: false, speaking: false }],
}

const row = (id: string, at: string, over: Record<string, unknown> = {}) => ({
  id,
  room: 'room-1',
  roomName: 'Standup',
  state: 'stopped',
  trigger: 'manual',
  startedBy: '7',
  startedAt: at,
  stoppedAt: new Date(Date.parse(at) + 724_000).toISOString(),
  transcriptReady: true,
  transcriptState: 'done',
  ...over,
})

const listCalls = (): unknown[][] => h.get.mock.calls.filter((c) => c[1] === 'voice/recordings')
const transcriptCalls = (): unknown[][] => h.get.mock.calls.filter((c) => c[1] === 'voice/transcript')

beforeEach(() => {
  h.get.mockReset()
  useRoomRecordingsSheet.setState({ open: null })
})
afterEach(cleanup)

describe('the recordings drawer', () => {
  it('Loading…, then the empty line', async () => {
    let answer: (v: unknown) => void = () => {}
    h.get.mockReturnValue(new Promise((r) => (answer = r)))
    render(<RecordingsDrawer target={target} onClose={() => {}} />)
    expect(screen.getByTestId('voice-recordings-loading').textContent).toBe('Loading…')
    await act(async () => answer({ ok: true, recordings: [], next: null, canRead: true }))
    expect(screen.getByTestId('voice-recordings-empty').textContent).toBe(NO_RECORDINGS_LINE)
    expect(screen.getByTestId('voice-recordings-heading').textContent).toBe('Recordings · Standup')
  })

  it('a failed read says so; nothing retries', async () => {
    h.get.mockRejectedValue(new DaemonHttpError('chat down', 503, 'chat_unavailable', { code: 'chat_unavailable', hint: 'this server’s chat didn’t answer; try again' }))
    render(<RecordingsDrawer target={target} onClose={() => {}} />)
    await waitFor(() => expect(screen.getByTestId('voice-recordings-error').textContent).toContain('Couldn’t read this room’s recordings.'))
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(listCalls()).toHaveLength(1)
  })

  it('newest first, as the DM’s card: length, when, who started it, the state', async () => {
    h.get.mockResolvedValue({
      ok: true,
      canRead: true,
      next: null,
      recordings: [row('1', '2026-11-17T10:00:00Z'), row('2', '2026-11-18T14:41:00Z', { transcriptState: 'running', transcriptReady: false }), row('3', '2026-11-16T09:00:00Z', { trigger: 'required', startedBy: null })],
    })
    render(<RecordingsDrawer target={target} onClose={() => {}} />)
    await waitFor(() => expect(screen.getAllByTestId('voice-recording')).toHaveLength(3))
    const rows = screen.getAllByTestId('voice-recording')
    expect(rows.map((r) => r.getAttribute('data-recording'))).toEqual(['2', '1', '3'])
    const cards = screen.getAllByTestId('home-chat-recording-card')
    expect(cards.map((c) => c.getAttribute('data-state'))).toEqual(['transcribing', 'ready', 'ready'])
    expect(within(rows[0]).getByTestId('home-chat-recording-card-title').textContent).toBe('Recording · 12:04')
    expect(within(rows[0]).getByTestId('home-chat-recording-card-detail').textContent).toBe('Started by Ada Lovelace')
    expect(within(rows[2]).getByTestId('home-chat-recording-card-detail').textContent).toBe('Started by the server’s rule')
    expect(within(rows[0]).queryByTestId('home-chat-show-transcript')).toBeNull()
    expect(within(rows[1]).getByTestId('home-chat-show-transcript').textContent).toBe('Show transcript')
    expect(screen.queryByTestId('voice-recordings-sign-in')).toBeNull()
    expect(listCalls()).toHaveLength(1)
  })

  it('a login that may list but not read (the bare owner token): no Show transcript, and a line saying how', async () => {
    h.get.mockResolvedValue({ ok: true, canRead: false, next: null, recordings: [row('1', '2026-11-17T10:00:00Z')] })
    render(<RecordingsDrawer target={target} onClose={() => {}} />)
    await waitFor(() => expect(screen.getByTestId('voice-recordings-sign-in').textContent).toBe(SIGN_IN_TO_READ_LINE))
    expect(screen.queryByTestId('home-chat-show-transcript')).toBeNull()
  })

  it('Show transcript opens the DM’s transcript panel, read page by page; Close goes back to the list', async () => {
    h.get.mockImplementation((_s: unknown, route: string, p: Record<string, unknown>) => {
      if (route === 'voice/recordings') return Promise.resolve({ ok: true, canRead: true, next: null, recordings: [row('42', '2026-11-18T14:41:00Z')] })
      if (p.after === undefined)
        return Promise.resolve({ ok: true, lines: [{ id: '1', speaker: 'owner', speakerExternalId: 'k2:owner', startMs: 0, text: 'Morning, all.' }, { id: '2', speaker: 'ana', speakerExternalId: 'k2:connect:u2', startMs: 4000, text: 'Hi, the build is green.' }], next: '2' })
      return Promise.resolve({ ok: true, lines: [{ id: '3', speaker: 'ana', speakerExternalId: 'k2:connect:u2', startMs: 9000, text: 'Shipping tonight.' }], next: null })
    })
    render(<RecordingsDrawer target={target} onClose={() => {}} />)
    await waitFor(() => expect(screen.getByTestId('home-chat-show-transcript')).toBeTruthy())
    fireEvent.click(screen.getByTestId('home-chat-show-transcript'))
    expect(screen.getByTestId('home-chat-transcript-state').textContent).toBe('Loading…')
    await waitFor(() => expect(screen.getAllByTestId('home-chat-transcript-turn')).toHaveLength(2))
    expect(screen.getByTestId('home-chat-transcript-heading').textContent).toMatch(/^Transcript · 12:04 · Nov 18, /)
    const turns = screen.getAllByTestId('home-chat-transcript-turn')
    expect(turns[0].textContent).toContain('owner')
    expect(turns[1].textContent).toContain('ana')
    expect(within(turns[1]).getAllByTestId('home-chat-transcript-line')).toHaveLength(2)
    expect(transcriptCalls().map((c) => c[2])).toEqual([
      { id: '42', limit: 200, after: undefined },
      { id: '42', limit: 200, after: '2' },
    ])
    // The panel's own search and copy.
    fireEvent.change(screen.getByTestId('home-chat-transcript-search'), { target: { value: 'green' } })
    expect(screen.getByTestId('home-chat-transcript-hits').textContent).toBe('1 match')
    expect(screen.getByTestId('home-chat-transcript-copy').textContent).toBe('Copy transcript')
    fireEvent.click(screen.getByTestId('home-chat-transcript-close'))
    expect(screen.queryByTestId('home-chat-transcript')).toBeNull()
    expect(screen.getAllByTestId('voice-recording')).toHaveLength(1)
    expect(listCalls()).toHaveLength(1)
  })

  it('a refused transcript read says why in the panel', async () => {
    h.get.mockImplementation((_s: unknown, route: string) =>
      route === 'voice/recordings'
        ? Promise.resolve({ ok: true, canRead: true, next: null, recordings: [row('42', '2026-11-18T14:41:00Z')] })
        : Promise.reject(new DaemonHttpError('no', 403, 'owner_token_cannot_read')),
    )
    render(<RecordingsDrawer target={target} onClose={() => {}} />)
    await waitFor(() => expect(screen.getByTestId('home-chat-show-transcript')).toBeTruthy())
    fireEvent.click(screen.getByTestId('home-chat-show-transcript'))
    await waitFor(() => expect(screen.getByTestId('home-chat-transcript-state').textContent).toContain('Sign in to this server with your own login'))
  })

  it('the sheet draws nothing until a room’s drawer opens; × and Escape close it', async () => {
    h.get.mockResolvedValue({ ok: true, recordings: [], next: null, canRead: true })
    render(<VoiceRoomRecordingsSheet />)
    expect(screen.queryByTestId('voice-recordings')).toBeNull()
    act(() => useRoomRecordingsSheet.getState().show(target))
    await waitFor(() => expect(screen.getByTestId('voice-recordings-empty')).toBeTruthy())
    fireEvent.keyDown(screen.getByTestId('voice-recordings-close'), { key: 'Escape' })
    expect(useRoomRecordingsSheet.getState().open).toBeNull()
    expect(screen.queryByTestId('voice-recordings')).toBeNull()
    act(() => useRoomRecordingsSheet.getState().show(target))
    await waitFor(() => expect(screen.getByTestId('voice-recordings-close')).toBeTruthy())
    fireEvent.click(screen.getByTestId('voice-recordings-close'))
    expect(screen.queryByTestId('voice-recordings')).toBeNull()
  })
})

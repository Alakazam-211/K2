// @vitest-environment jsdom
// OA2: Settings → Cores → Email Reader Log reads GET /cli/mail/reader/log
// with the filters and shows each read as a sentence (who did what in which
// mailbox, when) that opens to every detail; a daemon refusal shows the
// shared error card with its reason, an answer without rows says K2
// couldn't read it, never an empty log.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

const h = vi.hoisted(() => ({
  gets: [] as { route: string; params?: Record<string, unknown> }[],
  answer: null as unknown,
  fail: null as Error | null,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
    h.gets.push({ route, params })
    if (h.fail) throw h.fail
    return h.answer
  }),
}))

import { MailReaderLogSection, MAIL_READER_LOG_MANIFEST } from './MailReaderLogSection'
import { readerActionLabel, readerActionPhrase, readerActorLabel, readerActorName, readerFromText, readerMailboxText } from './mail-reader-log-api'

const ROWS = [
  { id: 7, at: 1793000000, actor: 'user:ann', actorRole: 'admin', source: 'hosted', address: 'pat@x.example', kind: 'person', own: false, action: 'reader/message', messageRef: 'm_1', reason: 'billing dispute', ipHint: 'tunnel 203.0.113.5', outcome: 'ok', status: 200 },
  { id: 8, at: 1793000100, actor: 'user:bo', actorRole: 'member', source: 'linked', address: 'pat@gmail.example', kind: 'person', own: false, action: 'read', messageRef: null, reason: null, ipHint: 'loopback 127.0.0.1', outcome: 'refused', status: 404 },
]

const DEFAULT = { inbox: undefined, actor: undefined, since: '7d', limit: 200 }

beforeEach(() => {
  h.gets.length = 0
  h.answer = { ok: true, count: 2, rows: ROWS }
  h.fail = null
})
afterEach(cleanup)

const pick = async (name: string, option: string): Promise<void> => {
  fireEvent.click(screen.getByRole('button', { name }))
  fireEvent.click(within(await screen.findByTestId('setting-dropdown-menu')).getByRole('button', { name: option }))
}

describe('Email Reader Log', () => {
  it('lists each read as a sentence, and opens one to every detail', async () => {
    render(<MailReaderLogSection />)
    await screen.findByTestId('reader-log-row-7')
    expect(h.gets[0]).toEqual({ route: 'mail/reader/log', params: DEFAULT })
    const a = screen.getByTestId('reader-log-row-7').textContent ?? ''
    expect(a).toContain('ann opened a message in pat@x.example')
    expect(a).toContain('Someone else’s mailbox on this server')
    expect(a).toContain('“billing dispute”')
    const b = screen.getByTestId('reader-log-row-8').textContent ?? ''
    expect(b).toContain('bo opened a message in pat@gmail.example')
    expect(b).toContain('From an agent tab')
    expect(b).toContain('Refused')
    expect(a).not.toContain('Refused')
    fireEvent.click(within(screen.getByTestId('reader-log-row-7')).getByRole('button'))
    const d = screen.getByTestId('reader-log-details-7').textContent ?? ''
    expect(d).toContain('ann, Admin')
    expect(d).toContain('The internet, through the tunnel (203.0.113.5)')
    expect(d).toContain('Allowed')
    fireEvent.click(within(screen.getByTestId('reader-log-row-8')).getByRole('button'))
    const r = screen.getByTestId('reader-log-details-8').textContent ?? ''
    expect(r).toContain('Refused: K2 didn’t open the mail')
    expect(r).not.toContain('404')
    expect(screen.getByTestId('reader-log-count').textContent).toBe('2 reads in the last 7 days')
  })

  it('an empty window says so in the shared empty state, with a way to see everything kept', async () => {
    h.answer = { ok: true, count: 0, rows: [] }
    render(<MailReaderLogSection />)
    const empty = await screen.findByTestId('reader-log-empty')
    expect(empty.textContent).toContain('No mailbox reads in the last 7 days')
    fireEvent.click(within(empty).getByText('Show everything kept'))
    await waitFor(() => expect(h.gets.at(-1)?.params).toEqual({ ...DEFAULT, since: undefined }))
  })

  it('filters by mailbox as you type, asks the server for a full address, and Show everyone clears', async () => {
    render(<MailReaderLogSection />)
    await screen.findByTestId('reader-log-row-7')
    fireEvent.change(screen.getByTestId('reader-log-inbox'), { target: { value: 'gmail' } })
    expect(screen.queryByTestId('reader-log-row-7')).toBeNull()
    expect(screen.getByTestId('reader-log-row-8')).toBeTruthy()
    fireEvent.change(screen.getByTestId('reader-log-inbox'), { target: { value: ' Pat@X.example ' } })
    await waitFor(() => expect(h.gets.at(-1)?.params).toEqual({ ...DEFAULT, inbox: 'pat@x.example' }), { timeout: 2000 })
    fireEvent.change(screen.getByTestId('reader-log-inbox'), { target: { value: 'nobody@else.example' } })
    expect((await screen.findByTestId('reader-log-no-match')).textContent).toContain('No reads of a mailbox matching “nobody@else.example” in the last 7 days')
    fireEvent.click(screen.getByTestId('reader-log-clear'))
    expect((screen.getByTestId('reader-log-inbox') as HTMLInputElement).value).toBe('')
    expect(await screen.findByTestId('reader-log-row-7')).toBeTruthy()
  })

  it('filters by who and how far back, on the server', async () => {
    render(<MailReaderLogSection />)
    await screen.findByTestId('reader-log-row-7')
    await pick('Who', 'ann')
    await waitFor(() => expect(h.gets.at(-1)?.params).toEqual({ ...DEFAULT, actor: 'user:ann' }))
    await pick('When', 'Last 30 days')
    await waitFor(() => expect(h.gets.at(-1)?.params).toEqual({ ...DEFAULT, actor: 'user:ann', since: '30d' }))
  })

  it('a refusal shows the plain line, the daemon’s words quietly, and no rows', async () => {
    h.fail = new Error('requires owner/admin')
    render(<MailReaderLogSection />)
    const card = await screen.findByTestId('reader-log-error-state')
    expect(card.getAttribute('data-load-state')).toBe('error')
    expect(screen.getByTestId('reader-log-error').textContent).toContain('K2 couldn’t read this server’s mailbox log')
    expect(card.textContent).toContain('The server said: requires owner/admin.')
    expect(screen.queryByTestId('reader-log-row-7')).toBeNull()
    h.fail = null
    await act(async () => {
      fireEvent.click(screen.getByText('Try again'))
    })
    expect(await screen.findByTestId('reader-log-row-7')).toBeTruthy()
  })

  it('an answer without rows is an error card, not an empty log or a crash', async () => {
    h.answer = { ok: true }
    render(<MailReaderLogSection />)
    expect((await screen.findByTestId('reader-log-error')).textContent).toContain('sent an answer K2 couldn’t read')
    expect(screen.queryByTestId('reader-log-empty')).toBeNull()
    cleanup()
    h.answer = {}
    render(<MailReaderLogSection />)
    expect(await screen.findByTestId('reader-log-error-state')).toBeTruthy()
  })

  it('words: actions, the owner token, mailboxes and where from; search finds it', () => {
    expect(readerActionLabel('reader/send')).toBe('Sent a message')
    expect(readerActionLabel('something/new')).toBe('something/new')
    expect(readerActionPhrase('reply')).toEqual({ phrase: 'replied', agentTab: true })
    expect(readerActionPhrase('something/new').phrase).toBe('did “something/new”')
    expect(readerActorLabel({ actor: 'owner-token', actorRole: 'owner' })).toBe('Server owner')
    expect(readerActorName('owner-token')).toBe('The server’s owner')
    expect(readerMailboxText({ kind: 'person', own: true, source: 'linked' })).toBe('Their own mailbox, a linked account')
    expect(readerFromText('lan -')).toBe('The local network')
    expect(readerFromText(null)).toBe('Not recorded')
    expect(MAIL_READER_LOG_MANIFEST[0]).toMatchObject({ section: 'email-reader-log', label: 'Email Reader Log' })
  })
})

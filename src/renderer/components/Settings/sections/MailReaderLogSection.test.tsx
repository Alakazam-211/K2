// @vitest-environment jsdom
// OA2: Settings → Cores → Email Reader Log reads GET /cli/mail/reader/log
// with the filters and shows each row in words (who, what, inbox, outcome,
// reason, where from); a daemon refusal shows its message, never an empty log.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

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
import { readerActionLabel, readerActorLabel } from './mail-reader-log-api'

const ROWS = [
  { id: 7, at: 1793000000, actor: 'user:ann', actorRole: 'admin', source: 'hosted', address: 'pat@x.example', kind: 'person', own: false, action: 'reader/message', messageRef: 'm_1', reason: 'billing dispute', ipHint: 'tunnel 203.0.113.5', outcome: 'ok', status: 200 },
  { id: 8, at: 1793000100, actor: 'user:bo', actorRole: 'member', source: 'linked', address: 'pat@gmail.example', kind: 'person', own: false, action: 'read', messageRef: null, reason: null, ipHint: 'loopback 127.0.0.1', outcome: 'refused', status: 404 },
]

beforeEach(() => {
  h.gets.length = 0
  h.answer = { ok: true, count: 2, rows: ROWS }
  h.fail = null
})
afterEach(cleanup)

describe('Email Reader Log', () => {
  it('lists the rows in words, newest filters sent to the reader log route', async () => {
    render(<MailReaderLogSection />)
    await screen.findByTestId('reader-log-row-7')
    expect(h.gets[0]).toEqual({ route: 'mail/reader/log', params: { inbox: undefined, actor: undefined, since: '7d', limit: 200 } })
    const a = screen.getByTestId('reader-log-row-7').textContent ?? ''
    expect(a).toContain('ann (admin)')
    expect(a).toContain('Opened a message')
    expect(a).toContain('pat@x.example')
    expect(a).toContain('a person’s')
    expect(a).toContain('allowed')
    expect(a).toContain('billing dispute')
    expect(a).toContain('tunnel 203.0.113.5')
    const b = screen.getByTestId('reader-log-row-8').textContent ?? ''
    expect(b).toContain('Opened a message (agent tab)')
    expect(b).toContain('refused')
  })

  it('an empty window says so in the shared empty state, and Show everyone clears the filters', async () => {
    h.answer = { ok: true, count: 0, rows: [] }
    render(<MailReaderLogSection />)
    expect((await screen.findByTestId('reader-log-empty')).textContent).toContain('in the last 7 days')
    fireEvent.change(screen.getByTestId('reader-log-inbox'), { target: { value: 'pat@x.example' } })
    fireEvent.click(screen.getByTestId('reader-log-apply'))
    await waitFor(() => expect(screen.getByTestId('reader-log-empty').textContent).toContain('Nothing matches these filters'))
    fireEvent.click(screen.getByTestId('reader-log-clear'))
    await waitFor(() => expect(h.gets.at(-1)?.params).toEqual({ inbox: undefined, actor: undefined, since: '7d', limit: 200 }))
  })

  it('filters by inbox and who', async () => {
    render(<MailReaderLogSection />)
    await screen.findByTestId('reader-log-row-7')
    fireEvent.change(screen.getByTestId('reader-log-inbox'), { target: { value: ' pat@x.example ' } })
    fireEvent.change(screen.getByTestId('reader-log-actor'), { target: { value: 'user:ann' } })
    fireEvent.click(screen.getByTestId('reader-log-apply'))
    await waitFor(() => expect(h.gets.at(-1)?.params).toEqual({ inbox: 'pat@x.example', actor: 'user:ann', since: '7d', limit: 200 }))
  })

  it('a refusal shows the daemon’s words, and no rows', async () => {
    h.fail = new Error('requires owner/admin')
    render(<MailReaderLogSection />)
    expect((await screen.findByTestId('reader-log-error')).textContent).toBe('requires owner/admin')
    expect(screen.getByTestId('reader-log-error-state').getAttribute('data-load-state')).toBe('error')
    expect(screen.queryByTestId('reader-log-row-7')).toBeNull()
  })

  it('an answer without rows is an error, not an empty log', async () => {
    h.answer = { ok: true }
    render(<MailReaderLogSection />)
    expect((await screen.findByTestId('reader-log-error')).textContent).toBe('the reader log answered without rows')
  })

  it('labels: unknown actions pass through, the owner token reads as words; search finds it', () => {
    expect(readerActionLabel('reader/send')).toBe('Sent a message')
    expect(readerActionLabel('something/new')).toBe('something/new')
    expect(readerActorLabel({ actor: 'owner-token', actorRole: 'owner' })).toBe('Owner token')
    expect(MAIL_READER_LOG_MANIFEST[0]).toMatchObject({ section: 'email-reader-log', label: 'Email Reader Log' })
  })
})

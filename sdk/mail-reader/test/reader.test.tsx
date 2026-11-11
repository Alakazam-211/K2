// @vitest-environment jsdom
// @k2/mail-reader behaviour with a recording fake source (prd-ocmail-admin-
// in-k2-v1 §12): T12 opening reads only (no write method), T13 HTML goes
// only through the host's renderHtml slot, T14 the role view, T15 nothing
// remote reaches the frame; plus the explicit write buttons and their asks.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import {
  MailReader,
  InboxList,
  groupInboxes,
  hasRemoteSrc,
  sanitizeEmailHtml,
  WRITE_METHODS,
  type MailReaderSource,
  type ReaderInbox,
  type ReaderMessage,
  type ReaderViewer,
} from '../src'

afterEach(cleanup)

const ADMIN: ReaderViewer = { actor: 'user:ann', role: 'admin', opensAny: true }
const MEMBER: ReaderViewer = { actor: 'user:pat', role: 'member', opensAny: false }

const INBOXES: ReaderInbox[] = [
  { address: 'ann@x.example', source: 'hosted', provider: 'hosted', kind: 'person', own: true, workspace: 'ann-ws', person: 'ann' },
  { address: 'bot@x.example', source: 'hosted', provider: 'hosted', kind: 'agent', own: false, workspace: 'sales-bot', person: null },
  { address: 'pat@gmail.example', source: 'linked', provider: 'gmail', kind: 'person', own: false, workspace: 'pat-ws', person: 'pat' },
]

const HOSTILE_HTML =
  '<p>Hello</p>' +
  '<img src="https://tracker.example/open.png" alt="t">' +
  '<img srcset="https://tracker.example/2x.png 2x" src="data:image/png;base64,AAAA">' +
  '<div style="background:url(http://tracker.example/bg.png);color:red">styled</div>' +
  '<table background="https://tracker.example/t.png"><tr><td>cell</td></tr></table>' +
  '<a href="https://evil.example/login">click</a>' +
  '<script>alert(1)</script>' +
  '<img src="x" onerror="alert(2)">' +
  '<link rel="stylesheet" href="https://tracker.example/s.css">' +
  '<video src="https://tracker.example/v.mp4"></video>'

function message(over: Partial<ReaderMessage> = {}): ReaderMessage {
  return {
    id: 'm_1',
    inbox: 'bot@x.example',
    from: { name: 'Sender', address: 's@o.example' },
    to: [{ name: '', address: 'bot@x.example' }],
    subject: 'Hello there',
    date: '2026-11-01T10:00:00Z',
    unread: true,
    hasAttachment: false,
    threadId: 't-1',
    text: 'plain body',
    html: null,
    attachments: [],
    authResults: { spf: 'pass', dkim: 'fail', dmarc: null },
    ...over,
  }
}

function fakeSource(opts: { viewer?: ReaderViewer; inboxes?: ReaderInbox[]; thread?: ReaderMessage[] } = {}) {
  const calls: { method: string; args: unknown[] }[] = []
  // Typed as the method it stands in for: records the call, answers `answer`.
  const rec = <K extends keyof MailReaderSource>(method: K, answer: Awaited<ReturnType<MailReaderSource[K]>>) =>
    vi.fn(async (...args: unknown[]) => {
      calls.push({ method, args })
      return answer
    }) as unknown as MailReaderSource[K]
  const thread = opts.thread ?? [message()]
  const source = {
    inboxes: rec('inboxes', { viewer: opts.viewer ?? ADMIN, inboxes: opts.inboxes ?? INBOXES }),
    folders: rec('folders', [
      { name: 'Inbox', role: 'inbox' },
      { name: 'Archive', role: 'archive' },
      { name: 'Receipts', role: null },
    ]),
    list: rec('list', { items: thread.filter((m) => !m.draft).map(({ id, inbox, from, to, subject, date, unread, hasAttachment, threadId }) => ({ id, inbox, from, to, subject, date, unread, hasAttachment, threadId })), nextOffset: null }),
    message: rec('message', thread[0]),
    thread: rec('thread', thread),
    attachment: rec('attachment', { filename: 'a.txt', mime: 'text/plain', size: 1, blob: new Blob(['a']) }),
    move: rec('move', { ok: true }),
    flag: rec('flag', { ok: true }),
    archive: rec('archive', { ok: true }),
    delete: rec('delete', { ok: true }),
    reply: rec('reply', { ok: true, status: 'submitted' }),
    draft: rec('draft', { ok: true }),
    send: rec('send', { ok: true, status: 'pending_approval' }),
    draftSave: rec('draftSave', { ok: true, draftId: 'd_1', draftStatus: 'edited' }),
    draftSend: rec('draftSend', { ok: true, status: 'submitted', draftId: 'd_1', draftStatus: 'sent' }),
    draftDiscard: rec('draftDiscard', { ok: true, draftId: 'd_1', draftStatus: 'discarded' }),
  } satisfies MailReaderSource
  return { source, calls, writes: () => calls.filter((c) => (WRITE_METHODS as readonly string[]).includes(c.method)) }
}

type HtmlSlot = (p: { html: string; title: string }) => React.ReactNode

async function openFirstThread(
  f: ReturnType<typeof fakeSource>,
  renderHtml: HtmlSlot = vi.fn(() => null),
  confirm: () => Promise<boolean> = vi.fn(async () => true),
) {
  render(<MailReader source={f.source} renderHtml={renderHtml} confirm={confirm} />)
  // Admin's own mailbox is picked first; switch to the agent inbox.
  fireEvent.click(await screen.findByTestId('mr-inbox-bot@x.example'))
  fireEvent.click(await screen.findByTestId('mr-thread-m_1'))
  await screen.findByTestId('mr-viewer')
  return { renderHtml, confirm }
}

describe('T12: opening reads only', () => {
  it('picking an inbox, a folder and a conversation calls no write method', async () => {
    const f = fakeSource()
    await openFirstThread(f)
    fireEvent.click(screen.getByTestId('mr-folder-archive'))
    await waitFor(() => expect(f.calls.filter((c) => c.method === 'list').length).toBeGreaterThanOrEqual(3))
    expect(f.calls.some((c) => c.method === 'thread')).toBe(true)
    expect(f.writes()).toEqual([])
  })

  it('Mark read is the only read-state change, and only when pressed', async () => {
    const f = fakeSource()
    await openFirstThread(f)
    expect(f.writes()).toEqual([])
    fireEvent.click(screen.getByTestId('mr-act-read'))
    await waitFor(() => expect(f.writes()).toEqual([{ method: 'flag', args: ['m_1', { read: true }] }]))
    await waitFor(() => expect(screen.getByTestId('mr-act-read').textContent).toBe('Mark unread'))
  })

  it('Delete and Send ask first; a no sends nothing', async () => {
    const f = fakeSource()
    const confirm = vi.fn(async () => false)
    await openFirstThread(f, vi.fn(() => null), confirm)
    fireEvent.click(screen.getByTestId('mr-act-delete'))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    fireEvent.click(screen.getByTestId('mr-act-reply'))
    fireEvent.change(screen.getByTestId('mr-compose-body'), { target: { value: 'thanks' } })
    fireEvent.click(screen.getByTestId('mr-compose-send'))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(2))
    expect(f.writes()).toEqual([])
  })

  it('a yes runs the write, and an approval-held send says so', async () => {
    const f = fakeSource()
    await openFirstThread(f)
    fireEvent.click(screen.getByTestId('mr-new-message'))
    fireEvent.change(screen.getByTestId('mr-compose-to'), { target: { value: 'a@b.example, c@d.example' } })
    fireEvent.change(screen.getByTestId('mr-compose-subject'), { target: { value: 'Hi' } })
    fireEvent.change(screen.getByTestId('mr-compose-body'), { target: { value: 'body text' } })
    fireEvent.click(screen.getByTestId('mr-compose-send'))
    await waitFor(() =>
      expect(f.writes()).toEqual([
        { method: 'send', args: [{ inbox: 'bot@x.example', to: ['a@b.example', 'c@d.example'], cc: [], subject: 'Hi', body: 'body text' }] },
      ]),
    )
    expect((await screen.findByTestId('mr-notice')).textContent).toBe('Waiting for approval before it goes out.')
  })
})

describe('T13: HTML only through the host slot', () => {
  it('an HTML message goes to renderHtml as a full document; the package renders no frame', async () => {
    const f = fakeSource({ thread: [message({ html: '<p>Hi <b>there</b></p>' })] })
    const renderHtml = vi.fn(({ html }: { html: string; title: string }) => <div data-testid="host-frame" data-len={html.length} />)
    await openFirstThread(f, renderHtml)
    expect(renderHtml).toHaveBeenCalled()
    const last = renderHtml.mock.calls[renderHtml.mock.calls.length - 1][0]
    expect(last.title).toBe('Hello there')
    expect(last.html.startsWith('<!doctype html>')).toBe(true)
    expect(last.html).toContain('<b>there</b>')
    expect(document.querySelector('iframe')).toBeNull()
    expect(screen.getByTestId('host-frame')).toBeTruthy()
  })

  it('a plain-text message never calls renderHtml', async () => {
    const f = fakeSource()
    const renderHtml = vi.fn(() => null)
    await openFirstThread(f, renderHtml)
    expect(renderHtml).not.toHaveBeenCalled()
    expect(screen.getByTestId('mr-text-body').textContent).toBe('plain body')
  })
})

describe('T15: nothing remote reaches the frame', () => {
  it('sanitizeEmailHtml drops remote src, srcset, background, url(), links, script and handlers', () => {
    const out = sanitizeEmailHtml(HOSTILE_HTML)
    expect(hasRemoteSrc(out)).toBe(false)
    expect(out).not.toMatch(/https?:\/\/[^"]*\.(png|css|mp4)/)
    expect(out).not.toMatch(/<script|onerror|<link|<video|\shref=/i)
    expect(out).toContain('Hello')
    expect(out).toContain('src="data:image/png;base64,AAAA"')
    expect(out).toContain('title="https://evil.example/login"')
    expect(out).toContain('styled')
  })

  it('the document handed to renderHtml has no http: or https: in any src', async () => {
    const f = fakeSource({ thread: [message({ html: HOSTILE_HTML })] })
    const renderHtml = vi.fn(() => null)
    await openFirstThread(f, renderHtml)
    const doc = (renderHtml.mock.calls[renderHtml.mock.calls.length - 1] as unknown as [{ html: string }])[0].html
    expect(doc).not.toMatch(/\s(?:src|srcset|background)\s*=\s*["']?\s*https?:/i)
    expect(doc).not.toMatch(/url\(\s*["']?\s*https?:/i)
    expect(hasRemoteSrc(doc)).toBe(false)
  })

  it('hasRemoteSrc catches what it guards (no false all-clear)', () => {
    expect(hasRemoteSrc('<img src="https://a.example/x.png">')).toBe(true)
    expect(hasRemoteSrc('<div style="background:url(//a.example/x.png)">')).toBe(true)
    expect(hasRemoteSrc('<img src="data:image/png;base64,AA">')).toBe(false)
  })
})

describe('T14: the role view', () => {
  it('a Member sees no "All inboxes" group, even if the list held other inboxes', () => {
    expect(groupInboxes(MEMBER, INBOXES).all).toBeNull()
    render(<InboxList viewer={MEMBER} inboxes={INBOXES} selected={null} onSelect={() => {}} />)
    expect(screen.queryByTestId('mr-group-all')).toBeNull()
    expect(screen.queryByText('All inboxes')).toBeNull()
    expect(screen.getByTestId('mr-group-own')).toBeTruthy()
    expect(screen.queryByTestId('mr-inbox-bot@x.example')).toBeNull()
    expect(screen.queryByTestId('mr-inbox-pat@gmail.example')).toBeNull()
  })

  it('an Admin sees "All inboxes" grouped by owner, with source labels', () => {
    const g = groupInboxes(ADMIN, INBOXES)
    expect(g.own.map((i) => i.address)).toEqual(['ann@x.example'])
    expect(g.all?.map((o) => o.owner)).toEqual(['pat', 'sales-bot'])
    render(<InboxList viewer={ADMIN} inboxes={INBOXES} selected={null} onSelect={() => {}} />)
    expect(screen.getByTestId('mr-group-all')).toBeTruthy()
    expect(screen.getByTestId('mr-owner-pat').textContent).toContain('Gmail · Person')
    expect(screen.getByTestId('mr-owner-sales-bot').textContent).toContain('Hosted · Agent')
  })

  it('a Member with no mailboxes gets the own-mailbox empty line, not the admin one', () => {
    render(<InboxList viewer={MEMBER} inboxes={[]} selected={null} onSelect={() => {}} />)
    expect(screen.getByText('You have no mailboxes on this server yet.')).toBeTruthy()
  })
})

describe('refresh', () => {
  it('window focus reloads the inboxes and the list (no push)', async () => {
    const f = fakeSource()
    render(<MailReader source={f.source} renderHtml={() => null} />)
    await screen.findByTestId('mr-inbox-bot@x.example')
    await waitFor(() => expect(f.calls.filter((c) => c.method === 'list').length).toBe(1))
    const before = f.calls.filter((c) => c.method === 'inboxes').length
    await act(async () => {
      window.dispatchEvent(new Event('focus'))
    })
    await waitFor(() => expect(f.calls.filter((c) => c.method === 'inboxes').length).toBe(before + 1))
    expect(f.writes()).toEqual([])
  })
})

/** An agent's unsent reply, as the daemon lists it first in a thread. */
function draft(over: Partial<ReaderMessage> = {}): ReaderMessage {
  return message({
    id: 'd_1',
    draft: true,
    draftId: 'd_1',
    from: { name: 'Sales Bot', address: 'bot@x.example' },
    to: [{ name: '', address: 's@o.example' }],
    cc: [{ name: '', address: 'bo@example.com' }],
    subject: 'Re: Hello there',
    text: 'agent words',
    unread: false,
    authResults: null,
    author: { kind: 'agent', name: 'sales-bot' },
    editedAt: 1_762_790_400,
    editedBy: null,
    editable: true,
    ...over,
  })
}

describe('drafted replies at the top of the thread', () => {
  it('the thread’s draft is a DraftCard above the conversation, not a message; opening writes nothing', async () => {
    const f = fakeSource({ thread: [draft(), message()] })
    await openFirstThread(f)
    const card = await screen.findByTestId('mr-draft-card')
    expect(card.textContent).toContain('Draft by sales-bot')
    expect(screen.getByTestId('mr-draft-unsent').textContent).toBe('not sent')
    expect((screen.getByTestId('mr-draft-body') as HTMLTextAreaElement).value).toBe('agent words')
    const viewer = screen.getByTestId('mr-viewer')
    expect(card.compareDocumentPosition(viewer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(screen.getAllByTestId('mr-viewer')).toHaveLength(1)
    expect(f.writes()).toEqual([])
  })

  it('an edit is saved once the person pauses, with the draft’s own To, Cc and Subject', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      const f = fakeSource({ thread: [draft(), message()] })
      await openFirstThread(f)
      await screen.findByTestId('mr-draft-card')
      fireEvent.change(screen.getByTestId('mr-draft-body'), { target: { value: 'better' } })
      fireEvent.change(screen.getByTestId('mr-draft-body'), { target: { value: 'better words' } })
      expect(f.writes()).toEqual([])
      await act(async () => {
        vi.advanceTimersByTime(1500)
      })
      await waitFor(() =>
        expect(f.writes()).toEqual([
          { method: 'draftSave', args: ['d_1', { to: ['s@o.example'], cc: ['bo@example.com'], subject: 'Re: Hello there', body: 'better words' }] },
        ]),
      )
    } finally {
      vi.useRealTimers()
    }
  })

  it('Send asks first; a no sends nothing, a yes sends the edited text and reloads the conversation', async () => {
    const f = fakeSource({ thread: [draft(), message()] })
    const answers = [false, true]
    const confirm = vi.fn(async () => answers.shift() ?? false)
    await openFirstThread(f, vi.fn(() => null), confirm)
    await screen.findByTestId('mr-draft-card')
    fireEvent.click(screen.getByTestId('mr-draft-send'))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(f.writes()).toEqual([])
    fireEvent.change(screen.getByTestId('mr-draft-body'), { target: { value: 'final words' } })
    fireEvent.click(screen.getByTestId('mr-draft-send'))
    await waitFor(() =>
      expect(f.writes()).toEqual([
        { method: 'draftSend', args: ['d_1', { to: ['s@o.example'], cc: ['bo@example.com'], subject: 'Re: Hello there', body: 'final words' }] },
      ]),
    )
    await waitFor(() => expect(f.calls.filter((c) => c.method === 'thread').length).toBe(2))
    expect((await screen.findByTestId('mr-notice')).textContent).toBe('Sent. The draft is now in the conversation.')
  })

  it('Discard asks first, then removes the card and leaves the conversation', async () => {
    const f = fakeSource({ thread: [draft(), message()] })
    const answers = [false, true]
    const confirm = vi.fn(async () => answers.shift() ?? false)
    await openFirstThread(f, vi.fn(() => null), confirm)
    await screen.findByTestId('mr-draft-card')
    fireEvent.click(screen.getByTestId('mr-draft-discard'))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(f.writes()).toEqual([])
    fireEvent.click(screen.getByTestId('mr-draft-discard'))
    await waitFor(() => expect(f.writes()).toEqual([{ method: 'draftDiscard', args: ['d_1'] }]))
    await waitFor(() => expect(screen.queryByTestId('mr-draft-card')).toBeNull())
    expect(screen.getByTestId('mr-viewer')).toBeTruthy()
  })

  it('an edited To and Subject are saved with the text, and a later text edit keeps them', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      const f = fakeSource({ thread: [draft(), message()] })
      await openFirstThread(f)
      await screen.findByTestId('mr-draft-card')
      expect(screen.queryByTestId('mr-draft-to')).toBeNull()
      fireEvent.click(screen.getByTestId('mr-draft-headers-toggle'))
      fireEvent.change(screen.getByTestId('mr-draft-to'), { target: { value: 's@o.example, sam@example.com' } })
      fireEvent.change(screen.getByTestId('mr-draft-subject'), { target: { value: 'Re: Hello there (23 of us)' } })
      await act(async () => {
        vi.advanceTimersByTime(1500)
      })
      const saved = { to: ['s@o.example', 'sam@example.com'], cc: ['bo@example.com'], subject: 'Re: Hello there (23 of us)' }
      await waitFor(() => expect(f.writes()).toEqual([{ method: 'draftSave', args: ['d_1', { ...saved, body: 'agent words' }] }]))
      fireEvent.change(screen.getByTestId('mr-draft-body'), { target: { value: 'new words' } })
      await act(async () => {
        vi.advanceTimersByTime(1500)
      })
      await waitFor(() => expect(f.writes()).toHaveLength(2))
      expect(f.writes()[1]).toEqual({ method: 'draftSave', args: ['d_1', { ...saved, body: 'new words' }] })
    } finally {
      vi.useRealTimers()
    }
  })

  it('a draft with no To saves nothing, and Send says what is missing', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      const f = fakeSource({ thread: [draft(), message()] })
      await openFirstThread(f)
      await screen.findByTestId('mr-draft-card')
      fireEvent.click(screen.getByTestId('mr-draft-headers-toggle'))
      fireEvent.change(screen.getByTestId('mr-draft-to'), { target: { value: '' } })
      await act(async () => {
        vi.advanceTimersByTime(1500)
      })
      expect(f.writes()).toEqual([])
      expect((screen.getByTestId('mr-draft-send') as HTMLButtonElement).disabled).toBe(true)
      expect(screen.getByTestId('mr-draft-blocker').textContent).toBe('Add a recipient to send.')
    } finally {
      vi.useRealTimers()
    }
  })

  it('a draft K2 can only show has no Send, Discard or fields, and says why in the daemon’s words', async () => {
    const f = fakeSource({ thread: [draft({ editable: false, hint: 'This draft can only be edited in Outlook.' }), message()] })
    await openFirstThread(f)
    const card = await screen.findByTestId('mr-draft-card')
    expect(card.getAttribute('data-readonly')).toBe('true')
    expect(screen.getByTestId('mr-draft-unsent').textContent).toBe('not sent')
    expect(screen.getByTestId('mr-draft-readonly').textContent).toBe('This draft can only be edited in Outlook.')
    expect(screen.getByTestId('mr-draft-body').textContent).toBe('agent words')
    expect(screen.queryByTestId('mr-draft-send')).toBeNull()
    expect(screen.queryByTestId('mr-draft-discard')).toBeNull()
    expect(card.querySelector('textarea, input')).toBeNull()
    fireEvent.click(screen.getByTestId('mr-draft-headers-toggle'))
    expect(screen.getByTestId('mr-draft-to').textContent).toBe('s@o.example')
    expect(card.querySelector('textarea, input')).toBeNull()
    expect(f.writes()).toEqual([])
  })

  it('a draft K2 can’t read shows no recipients line and says its text isn’t available', async () => {
    const f = fakeSource({ thread: [draft({ editable: false, to: [], cc: [], subject: null, text: null }), message()] })
    await openFirstThread(f)
    await screen.findByTestId('mr-draft-card')
    expect(screen.queryByTestId('mr-draft-headers-toggle')).toBeNull()
    expect(screen.getByTestId('mr-draft-body').textContent).toBe('K2 can’t show this draft’s text.')
  })

  it('without the daemon’s words, a read-only draft names its provider’s app', async () => {
    const f = fakeSource({
      inboxes: [{ ...INBOXES[1], provider: 'microsoft', source: 'linked' }],
      thread: [draft({ editable: false, hint: null }), message()],
    })
    render(<MailReader source={f.source} renderHtml={() => null} confirm={async () => true} />)
    fireEvent.click(await screen.findByTestId('mr-thread-m_1'))
    expect((await screen.findByTestId('mr-draft-readonly')).textContent).toBe('This draft can only be edited in Outlook.')
  })
})

// @vitest-environment jsdom
// OA6: the reader's layout and controls. Narrow readers show one column at a
// time with Back; the columns' widths fit the reader; the composer sends on
// ⌘↩ and asks before Esc throws text away; auth badges say what they mean.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import {
  Composer,
  LIST,
  MESSAGE_MIN,
  MailReader,
  SIDEBAR,
  authTitle,
  authTone,
  fitPanes,
  type MailReaderSource,
  type ReaderMessage,
} from '../src'

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

function msg(over: Partial<ReaderMessage> = {}): ReaderMessage {
  return {
    id: 'm_1',
    inbox: 'me@x.example',
    from: { name: 'Sender', address: 's@o.example' },
    to: [{ name: '', address: 'me@x.example' }],
    subject: 'Hello',
    date: '2026-11-01T10:00:00Z',
    unread: false,
    hasAttachment: false,
    threadId: 't-1',
    text: 'body',
    html: null,
    attachments: [],
    authResults: null,
    ...over,
  }
}

function source(thread: ReaderMessage[] = [msg()]) {
  const reply = vi.fn(async () => ({ ok: true }))
  const s: MailReaderSource = {
    inboxes: async () => ({
      viewer: { actor: 'user:me', role: 'owner', opensAny: false },
      inboxes: [{ address: 'me@x.example', source: 'hosted', provider: 'hosted', kind: 'person', own: true }],
    }),
    folders: async () => [{ name: 'Inbox', role: 'inbox' }, { name: 'Receipts', role: null }],
    list: async () => ({ items: thread, nextOffset: null }),
    message: async () => thread[0],
    thread: async () => thread,
    attachment: async () => ({ filename: 'a', mime: 'text/plain', size: 1, blob: new Blob(['a']) }),
    move: async () => ({}),
    flag: async () => ({}),
    archive: async () => ({}),
    delete: async () => ({}),
    reply,
    draft: async () => ({}),
    send: async () => ({}),
  }
  return { s, reply }
}

/** The reader measures itself; jsdom has no layout, so give it a width. */
function readerWidth(px: number): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const w = this.getAttribute('data-mr') === 'reader' ? px : 0
    return { width: w, height: 600, top: 0, left: 0, right: w, bottom: 600, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
  })
}

describe('narrow: one column at a time', () => {
  it('below 840px: list → conversation → Back → list → Back → mailboxes', async () => {
    readerWidth(700)
    const { s } = source()
    render(<MailReader source={s} renderHtml={() => null} />)
    const reader = await screen.findByTestId('mr-reader')
    expect(reader.getAttribute('data-layout')).toBe('stacked')
    fireEvent.click(await screen.findByTestId('mr-thread-m_1'))
    await screen.findByTestId('mr-viewer')
    expect(document.querySelector('[data-mr="list-pane"]')).toBeNull()
    expect(document.querySelector('[data-mr="sidebar"]')).toBeNull()
    fireEvent.click(screen.getByTestId('mr-back'))
    expect(reader.getAttribute('data-view')).toBe('list')
    expect(screen.queryByTestId('mr-viewer')).toBeNull()
    fireEvent.click(screen.getByTestId('mr-back'))
    expect(reader.getAttribute('data-view')).toBe('folders')
    // Picking a folder goes back to the list.
    fireEvent.click(await screen.findByTestId('mr-folder-inbox'))
    expect(reader.getAttribute('data-view')).toBe('list')
  })

  it('at 840px and wider: three columns, two handles, no Back', async () => {
    readerWidth(1200)
    const { s } = source()
    render(<MailReader source={s} renderHtml={() => null} />)
    const reader = await screen.findByTestId('mr-reader')
    expect(reader.getAttribute('data-layout')).toBe('columns')
    expect(screen.getByTestId('mr-resize-sidebar').getAttribute('role')).toBe('separator')
    expect(screen.getByTestId('mr-resize-list')).toBeTruthy()
    expect(screen.queryByTestId('mr-back')).toBeNull()
  })

  it('the handles move with the arrow keys within their range', async () => {
    readerWidth(1400)
    const { s } = source()
    render(<MailReader source={s} renderHtml={() => null} />)
    const h = await screen.findByTestId('mr-resize-list')
    const before = Number(h.getAttribute('aria-valuenow'))
    fireEvent.keyDown(h, { key: 'ArrowRight' })
    await waitFor(() => expect(Number(screen.getByTestId('mr-resize-list').getAttribute('aria-valuenow'))).toBe(before + 16))
    expect(screen.getByTestId('mr-resize-list').getAttribute('aria-valuemax')).toBe(String(LIST.max))
  })
})

describe('fitPanes', () => {
  it('keeps each column in range and the conversation at least MESSAGE_MIN; the list gives way first', () => {
    expect(fitPanes({ sidebar: 999, list: 1 }, 2000)).toEqual({ sidebar: SIDEBAR.max, list: LIST.min })
    const tight = fitPanes({ sidebar: 300, list: 500 }, 900)
    expect(tight.sidebar + tight.list + MESSAGE_MIN).toBeLessThanOrEqual(900)
    expect(tight.list).toBe(LIST.min)
    expect(tight.sidebar).toBe(900 - MESSAGE_MIN - LIST.min)
  })
})

describe('auth badges', () => {
  it('pass is ok, fail is danger, anything else a warning; the tooltip says what it means', async () => {
    expect([authTone('pass'), authTone('FAIL'), authTone('permerror'), authTone('softfail'), authTone('none')]).toEqual(['ok', 'danger', 'danger', 'warn', 'warn'])
    expect(authTitle('SPF', 'softfail')).toMatch(/^SPF: .*\nNot confirmed \(softfail\)/)
    const { s } = source([msg({ authResults: { spf: 'softfail', dkim: 'pass', dmarc: 'fail' } })])
    render(<MailReader source={s} renderHtml={() => null} />)
    fireEvent.click(await screen.findByTestId('mr-thread-m_1'))
    expect((await screen.findByTestId('mr-auth-spf')).getAttribute('data-tone')).toBe('warn')
    expect(screen.getByTestId('mr-auth-dkim').getAttribute('data-tone')).toBe('ok')
    expect(screen.getByTestId('mr-auth-dmarc').getAttribute('data-tone')).toBe('danger')
    expect(screen.getByTestId('mr-auth-dmarc').getAttribute('title')).toContain('Failed (fail)')
  })
})

describe('reply all', () => {
  it('shows with more than one recipient and replies with all: true', async () => {
    const thread = [msg({ to: [{ address: 'me@x.example' }, { address: 'ann@o.example' }], cc: [{ address: 'bo@o.example' }] })]
    const { s, reply } = source(thread)
    render(<MailReader source={s} renderHtml={() => null} confirm={async () => true} />)
    fireEvent.click(await screen.findByTestId('mr-thread-m_1'))
    fireEvent.click(await screen.findByTestId('mr-act-reply-all'))
    expect((screen.getByTestId('mr-compose-to') as HTMLInputElement).value).toBe('s@o.example, ann@o.example, bo@o.example')
    fireEvent.change(screen.getByTestId('mr-compose-body'), { target: { value: 'hi all' } })
    fireEvent.click(screen.getByTestId('mr-compose-send'))
    await waitFor(() => expect(reply).toHaveBeenCalledWith({ id: 'm_1', body: 'hi all', all: true }))
  })
})

describe('composer keys', () => {
  const mode = { kind: 'new', inbox: 'me@x.example' } as const

  it('⌘↩ and Ctrl+Enter send when it can', () => {
    const onSend = vi.fn()
    render(<Composer mode={mode} busy={false} onSend={onSend} onSaveDraft={() => {}} onCancel={() => {}} />)
    fireEvent.change(screen.getByTestId('mr-compose-to'), { target: { value: 'a@b.example' } })
    fireEvent.change(screen.getByTestId('mr-compose-subject'), { target: { value: 'Hi' } })
    fireEvent.keyDown(screen.getByTestId('mr-compose-body'), { key: 'Enter', metaKey: true })
    expect(onSend).not.toHaveBeenCalled() // no body yet
    fireEvent.change(screen.getByTestId('mr-compose-body'), { target: { value: 'text' } })
    fireEvent.keyDown(screen.getByTestId('mr-compose-body'), { key: 'Enter', metaKey: true })
    fireEvent.keyDown(screen.getByTestId('mr-compose-body'), { key: 'Enter', ctrlKey: true })
    expect(onSend).toHaveBeenCalledTimes(2)
    expect(onSend.mock.calls[0][0]).toEqual({ to: ['a@b.example'], cc: [], subject: 'Hi', body: 'text' })
  })

  it('Esc closes at once when nothing is written, and asks first when something is', async () => {
    const onCancel = vi.fn()
    const confirm = vi.fn(async () => false)
    render(<Composer mode={mode} busy={false} onSend={() => {}} onSaveDraft={() => {}} onCancel={onCancel} confirm={confirm} />)
    fireEvent.keyDown(screen.getByTestId('mr-compose-body'), { key: 'Escape' })
    await waitFor(() => expect(onCancel).toHaveBeenCalledTimes(1))
    expect(confirm).not.toHaveBeenCalled()
    fireEvent.change(screen.getByTestId('mr-compose-body'), { target: { value: 'half a thought' } })
    fireEvent.keyDown(screen.getByTestId('mr-compose-body'), { key: 'Escape' })
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(onCancel).toHaveBeenCalledTimes(1) // the no kept it open
    expect(confirm.mock.calls[0]).toEqual([expect.objectContaining({ title: 'Discard this message?', destructive: true })])
  })

  it('Attach is off and says why when the host can’t attach', () => {
    render(<Composer mode={mode} busy={false} onSend={() => {}} onSaveDraft={() => {}} onCancel={() => {}} attachments={[{ id: 'f1', name: 'map.pdf', size: 2048 }]} />)
    const attach = screen.getByTestId('mr-compose-attach') as HTMLButtonElement
    expect(attach.disabled).toBe(true)
    expect(attach.getAttribute('title')).toBe('Attaching files from the reader isn’t available yet.')
    expect(document.querySelector('[data-mr="compose-attachment"]')?.textContent).toContain('map.pdf')
  })
})

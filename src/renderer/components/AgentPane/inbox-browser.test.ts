import { describe, it, expect } from 'vitest'
import {
  EXTERNAL_EMAIL_BEGIN,
  EXTERNAL_EMAIL_END,
  MAIL_PAGE_LIMIT,
  buildSourceList,
  chatAboutOpenLine,
  chatAboutRowTitle,
  chatAboutStampLine,
  formatChatAboutPayload,
  formatInboxError,
  mailHtmlSrcDoc,
  oneLineInboxTitle,
  parseMailCatalog,
  parseMailMessages,
  parseMailRead,
  parseTrayList,
  parseTrayRead,
  preferMailHtml,
  stripExternalEmailMarkers,
  traySource,
} from './inbox-browser'

describe('inbox-browser helpers', () => {
  it('always prefixes K2 Inbox and never emits a Postal row', () => {
    const catalog = parseMailCatalog({
      ok: true,
      inboxes: [
        { address: 'you@host.k2', source: 'hosted' },
        { address: 'you@gmail.com', source: 'linked' },
        { address: 'postal-bot', source: 'postal' },
      ],
    })
    const sources = buildSourceList(catalog)
    expect(sources[0]).toEqual(traySource())
    expect(sources.map((s) => s.kind)).toEqual(['tray', 'hosted', 'linked'])
    expect(sources.some((s) => s.label === 'Postal' || s.id === 'postal-bot')).toBe(false)
    expect(sources.find((s) => s.address === 'you@host.k2')?.tag).toBe('Hosted')
    expect(sources.find((s) => s.address === 'you@gmail.com')?.tag).toBe('Linked')
  })

  it('fails loud when mail catalog is not { inboxes: [...] }', () => {
    expect(() => parseMailCatalog([])).toThrow(/mail\/inboxes/)
    expect(() => parseMailCatalog({ ok: true })).toThrow(/mail\/inboxes/)
  })

  it('fails loud when tray list is not a JSON array (including {items:[…]})', () => {
    expect(() => parseTrayList({ items: [] })).toThrow(/inbox\/list/)
    expect(() => parseTrayList(undefined)).toThrow(/inbox\/list/)
    expect(parseTrayList([])).toEqual([])
  })

  it('reads tray body from { content }, not a raw string', () => {
    expect(parseTrayRead({ id: 'pkg-1', content: '# hi' }).content).toBe('# hi')
    expect(() => parseTrayRead('# hi')).toThrow(/inbox\/read/)
  })

  it('parses mail messages with nextOffset and inboxErrors', () => {
    const page = parseMailMessages({
      ok: true,
      messages: [{ id: 'm1', address: 'a@b.c', subject: 'Hi' }],
      nextOffset: 50,
      inboxErrors: [{ hint: 'gmail timed out' }],
    })
    expect(page.messages).toHaveLength(1)
    expect(page.nextOffset).toBe(50)
    expect(page.inboxErrors).toEqual(['gmail timed out'])
    expect(MAIL_PAGE_LIMIT).toBe(50)
    expect(() => parseMailMessages({ ok: true })).toThrow(/mail\/messages/)
    expect(() => parseMailMessages({ ok: false, error: { hint: 'engine down' } })).toThrow(/engine down/)
  })

  it('parses mail/read { message } and strips markers from HTML srcDoc', () => {
    const html = `${EXTERNAL_EMAIL_BEGIN}\n<p><a href="javascript:alert(1)">x</a></p>\n${EXTERNAL_EMAIL_END}`
    const msg = parseMailRead({
      ok: true,
      message: { id: 'm1', text: `${EXTERNAL_EMAIL_BEGIN}\nhello\n${EXTERNAL_EMAIL_END}`, html },
    })
    const src = mailHtmlSrcDoc(msg.html ?? '')
    expect(src).not.toContain(EXTERNAL_EMAIL_BEGIN)
    expect(src).not.toContain(EXTERNAL_EMAIL_END)
    expect(src).not.toMatch(/javascript:/i)
    expect(src).toContain('<p>')
    expect(() => parseMailRead({ ok: true })).toThrow(/mail\/read/)
  })

  it('prefers text over HTML unless text is empty', () => {
    expect(preferMailHtml('hello', '<p>x</p>', false)).toBe(false)
    expect(preferMailHtml('hello', '<p>x</p>', true)).toBe(true)
    expect(preferMailHtml(`${EXTERNAL_EMAIL_BEGIN}\n\n${EXTERNAL_EMAIL_END}`, '<p>x</p>', false)).toBe(true)
    expect(preferMailHtml('hello', null, false)).toBe(false)
  })

  it('keeps marker lines in the text path helper output (strip is opt-in)', () => {
    const raw = `${EXTERNAL_EMAIL_BEGIN}\nhello\n${EXTERNAL_EMAIL_END}`
    expect(stripExternalEmailMarkers(raw).inner.trim()).toBe('hello')
    expect(stripExternalEmailMarkers(raw).hadMarkers).toBe(true)
  })

  it('keeps leftover folder names on tray items without inventing status chips', () => {
    const items = parseTrayList([
      { id: 'a', folder: '', title: 'root' },
      { id: 'b', folder: 'active', title: 'was active' },
      { id: 'c', folder: 'done', title: 'was done' },
    ])
    expect(items.map((i) => i.folder)).toEqual(['', 'active', 'done'])
  })

  it('formats nested mail error JSON as a one-line hint', () => {
    expect(formatInboxError(new Error(JSON.stringify({
      ok: false,
      error: { code: 'engine', hint: 'IMAP auth failed' },
    })))).toBe('IMAP auth failed')
    expect(formatInboxError(new Error('boom'))).toBe('boom')
  })

  it('collapses CR/LF in Chat about this titles to one line', () => {
    expect(oneLineInboxTitle('Wake\r\npackage')).toBe('Wake package')
    expect(oneLineInboxTitle('Hello\n\nhost', '(no subject)')).toBe('Hello host')
    expect(oneLineInboxTitle('  \n  ', '(no subject)')).toBe('(no subject)')
    expect(chatAboutRowTitle('tray', { title: '', filename: 'pkg-1.md' })).toBe('pkg-1.md')
    expect(chatAboutRowTitle('hosted', { subject: null })).toBe('(no subject)')
  })

  it('stamps tray/hosted/linked without a legacy [inbox:] token', () => {
    const tray = formatChatAboutPayload(
      { kind: 'tray', id: 'pkg-1', title: 'Wake\npackage' },
      'look at this',
    )
    expect(tray).toBe(
      '[k2 inboxID:pkg-1] Wake package\nOpen: k2 inbox read pkg-1\n\nlook at this',
    )
    expect(chatAboutStampLine({ kind: 'tray', id: 'pkg-1', title: 'Wake package' }))
      .toBe('[k2 inboxID:pkg-1] Wake package')
    expect(chatAboutOpenLine({ kind: 'tray', id: 'pkg-1', title: 'Wake package' }))
      .toBe('Open: k2 inbox read pkg-1')
    expect(tray).not.toMatch(/\[inbox:/)
    expect(tray.split('\n')[0]).not.toMatch(/[\r\n]/)

    const hosted = formatChatAboutPayload(
      { kind: 'hosted', id: 'm_host', title: 'Hello host' },
      'note',
    )
    expect(hosted).toBe(
      '[hosted inboxID:m_host] Hello host\nOpen: k2 mail read m_host\n\nnote',
    )

    const linked = formatChatAboutPayload(
      { kind: 'linked', id: 'm_link', title: '' },
      'note',
    )
    expect(linked).toBe(
      '[linked inboxID:m_link] (no subject)\nOpen: k2 mail read m_link\n\nnote',
    )
    expect(hosted).not.toMatch(/\[inbox:/)
    expect(linked).not.toMatch(/\[inbox:/)
  })
})

// @vitest-environment jsdom
// A mail notice is attacker-controlled text. Drawn plainly: no Markdown, no
// links, no images, whatever the subject says (vs-live W34).

import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { coerceNotice, NoticeCard, type ThreadNoticeDoc } from './NoticeCard'

const notice: ThreadNoticeDoc = {
  source: 'mail',
  inbox: 'sales@acme.test',
  fromName: 'Jane Doe',
  fromAddr: 'jane@acme.test',
  auth: 'DMARC pass',
  subject: '[click](https://evil.test) <https://evil.test> ![x](https://t.test)',
  snippet: '**bold** and <img src=x onerror=alert(1)>',
  readCommand: 'k2 mail read m_8f2c',
  delivered: true,
  reason: null,
  noticeId: 7,
}

afterEach(() => cleanup())

describe('NoticeCard', () => {
  it('draws mail fields as literal text: no link, no image, no markup', () => {
    const { container } = render(<NoticeCard notice={notice} />)
    expect(container.querySelector('a')).toBeNull()
    expect(container.querySelector('img')).toBeNull()
    expect(container.querySelector('strong')).toBeNull()
    expect(container.querySelector('[data-notice-subject]')?.textContent).toBe(
      '«[click](https://evil.test) <https://evil.test> ![x](https://t.test)»',
    )
    expect(container.querySelector('[data-notice-snippet]')?.textContent).toBe('«**bold** and <img src=x onerror=alert(1)>»')
  })

  it('says it is external mail, and how to read it', () => {
    render(<NoticeCard notice={notice} />)
    expect(screen.getByText(/External email: shown as data/)).toBeTruthy()
    expect(screen.getByText(/k2 mail read m_8f2c/)).toBeTruthy()
  })

  it('says when the agent was not reached', () => {
    render(<NoticeCard notice={{ ...notice, delivered: false, reason: 'pty_stalled' }} />)
    expect(screen.getByText(/Couldn't reach the agent \(pty_stalled\)/)).toBeTruthy()
    expect(screen.getByText(/next digest/)).toBeTruthy()
  })

  it('reads a wire doc, and refuses one that is not a notice', () => {
    const wire = { ...notice, noticeId: 9 }
    expect(coerceNotice(wire)?.noticeId).toBe(9)
    expect(coerceNotice({ kind: 'nope' })).toBeNull()
    expect(coerceNotice(null)).toBeNull()
    expect(coerceNotice([])).toBeNull()
  })
})

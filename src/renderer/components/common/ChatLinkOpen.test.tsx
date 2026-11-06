// @vitest-environment jsdom
// GH#77: a link in a Thread message (chat markdown) opens in the default
// browser on click, through the opener plugin. Only http, https and mailto
// open; the click never reaches the message around the link.
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'

const opener = vi.hoisted(() => ({ openUrl: vi.fn<(url: string) => Promise<void>>() }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (url: string) => opener.openUrl(url) }))

const offOrigin = vi.hoisted(() => ({ openOffOriginHttp: vi.fn<(href: string) => void>() }))
vi.mock('@/lib/open-off-origin-http', () => ({ openOffOriginHttp: (href: string) => offOrigin.openOffOriginHttp(href) }))

const web = vi.hoisted(() => ({ on: false }))
vi.mock('@/lib/is-web', () => ({ isWebClient: () => web.on }))

vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn(async () => undefined) }))

import { ChatMessageBody } from './ChatMessage'
import { ChatLink } from './chat-copy'
import { installExternalLinkHandler } from '@/lib/external-link-handler'
import { externalLinkUrl } from '@/lib/open-external-link'
import { useToastStore } from '@/stores/toast'
import { useChatLinkCopyStore } from '@/stores/chat-link-copy'

function anchors(container: HTMLElement): HTMLAnchorElement[] {
  return Array.from(container.querySelectorAll('a'))
}

function onlyAnchor(container: HTMLElement): HTMLAnchorElement {
  const all = anchors(container)
  if (all.length !== 1) throw new Error(`expected one link, found ${all.length}`)
  return all[0]
}

/** A real click event, so `defaultPrevented` can be read afterwards. */
function click(el: Element, init: MouseEventInit = {}): MouseEvent {
  const ev = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0, ...init })
  act(() => {
    el.dispatchEvent(ev)
  })
  return ev
}

function toasts(): string[] {
  return useToastStore.getState().toasts.map((t) => t.message)
}

describe('chat links open outside K2 (GH#77)', () => {
  beforeAll(() => {
    // The app installs this at boot; chat links must not go through it too.
    installExternalLinkHandler()
  })

  beforeEach(() => {
    web.on = false
    opener.openUrl.mockReset()
    opener.openUrl.mockResolvedValue(undefined)
    offOrigin.openOffOriginHttp.mockReset()
    act(() => useToastStore.setState({ toasts: [] }))
  })

  afterEach(() => {
    cleanup()
    act(() => useChatLinkCopyStore.getState().setEnabled(false))
  })

  it('a markdown link click opens its URL in the default browser, once, and not in the in-app Browser tab', () => {
    const outer = vi.fn()
    const { container } = render(
      <div onClick={outer}>
        <ChatMessageBody text={'Test: [example](https://example.com/docs?a=1#top)'} />
      </div>,
    )
    const link = onlyAnchor(container)
    expect(link.getAttribute('href')).toBe('https://example.com/docs?a=1#top')
    const ev = click(link)
    expect(ev.defaultPrevented).toBe(true)
    expect(opener.openUrl.mock.calls).toEqual([['https://example.com/docs?a=1#top']])
    expect(offOrigin.openOffOriginHttp).not.toHaveBeenCalled()
    // The message around the link takes no action on the click.
    expect(outer).not.toHaveBeenCalled()
  })

  it('a bare URL is linkified and opens on click', () => {
    const { container } = render(<ChatMessageBody text={'Test: https://example.com and more'} />)
    const link = onlyAnchor(container)
    expect(link.getAttribute('href')).toBe('https://example.com')
    expect(link.textContent).toBe('https://example.com')
    click(link)
    expect(opener.openUrl.mock.calls).toEqual([['https://example.com/']])
  })

  it('the report\'s own message: both links open, each with its own URL', () => {
    const { container } = render(
      <ChatMessageBody text={'Test: http://example.com/a and [example](https://example.com/b)'} />,
    )
    const links = anchors(container)
    expect(links.length).toBe(2)
    click(links[0])
    click(links[1])
    expect(opener.openUrl.mock.calls).toEqual([['http://example.com/a'], ['https://example.com/b']])
  })

  it('a click on the link text inside the anchor (emphasis) still opens it', () => {
    const { container } = render(<ChatMessageBody text={'[**bold** link](https://example.com/x)'} />)
    const strong = container.querySelector('a strong')
    if (!strong) throw new Error('no emphasis inside the link')
    click(strong)
    expect(opener.openUrl.mock.calls).toEqual([['https://example.com/x']])
  })

  it('mailto opens (the default mail app)', () => {
    const { container } = render(<ChatMessageBody text={'[mail us](mailto:hello@example.com)'} />)
    click(onlyAnchor(container))
    expect(opener.openUrl.mock.calls).toEqual([['mailto:hello@example.com']])
  })

  it('a javascript: link in markdown never opens and never runs', () => {
    const { container } = render(<ChatMessageBody text={'[x](javascript:alert(1))'} />)
    const link = onlyAnchor(container)
    // react-markdown's transform (components/Markdown) drops the URL.
    expect(link.getAttribute('href') ?? '').toBe('')
    click(link)
    expect(opener.openUrl).not.toHaveBeenCalled()
    expect(offOrigin.openOffOriginHttp).not.toHaveBeenCalled()
  })

  it('a javascript: href reaching the link component is refused: default prevented, nothing opened, a toast says why', () => {
    const { container } = render(<ChatLink href="javascript:alert(1)">x</ChatLink>)
    const ev = click(onlyAnchor(container))
    expect(ev.defaultPrevented).toBe(true)
    expect(opener.openUrl).not.toHaveBeenCalled()
    expect(offOrigin.openOffOriginHttp).not.toHaveBeenCalled()
    expect(toasts()).toEqual(['K2 opens only http, https and mailto links.'])
  })

  it.each([
    ['file:///etc/passwd'],
    ['vscode://file/etc/hosts'],
    ['docs/readme.md'],
    ['/cli/status'],
    ['#section'],
  ])('%s is refused and the webview does not follow it', (href) => {
    const { container } = render(<ChatMessageBody text={`[x](${href})`} />)
    const link = onlyAnchor(container)
    expect(link.getAttribute('href')).toBe(href)
    const ev = click(link)
    expect(ev.defaultPrevented).toBe(true)
    expect(opener.openUrl).not.toHaveBeenCalled()
    expect(offOrigin.openOffOriginHttp).not.toHaveBeenCalled()
  })

  it('a failure from the OS opener is shown, not swallowed', async () => {
    opener.openUrl.mockRejectedValueOnce(new Error('not allowed'))
    const { container } = render(<ChatMessageBody text={'[x](https://example.com/)'} />)
    click(onlyAnchor(container))
    await act(async () => {
      await Promise.resolve()
    })
    expect(toasts()).toEqual(["Couldn't open the link: not allowed"])
  })

  it('a middle click opens the link too', () => {
    const { container } = render(<ChatMessageBody text={'[x](https://example.com/m)'} />)
    const ev = new MouseEvent('auxclick', { bubbles: true, cancelable: true, button: 1 })
    act(() => {
      onlyAnchor(container).dispatchEvent(ev)
    })
    expect(ev.defaultPrevented).toBe(true)
    expect(opener.openUrl.mock.calls).toEqual([['https://example.com/m']])
  })

  it('with the copy button beside links on, a click on the link still opens it, and the copy button does not', () => {
    act(() => useChatLinkCopyStore.getState().setEnabled(true))
    const { container } = render(<ChatMessageBody text={'[x](https://example.com/c)'} />)
    const btn = container.querySelector('[data-chat-copy="link"]')
    if (!(btn instanceof HTMLButtonElement)) throw new Error('link has no copy icon')
    click(btn)
    expect(opener.openUrl).not.toHaveBeenCalled()
    click(container.querySelector('a[data-chat-link]') as HTMLElement)
    expect(opener.openUrl.mock.calls).toEqual([['https://example.com/c']])
  })

  it('hosted web: an http link follows its own target=_blank; K2 opens nothing itself', () => {
    web.on = true
    const outer = vi.fn()
    const { container } = render(
      <div onClick={outer}>
        <ChatMessageBody text={'[x](https://example.com/w)'} />
      </div>,
    )
    const link = onlyAnchor(container)
    expect(link.getAttribute('target')).toBe('_blank')
    expect(link.getAttribute('rel')).toBe('noopener noreferrer')
    // jsdom does not navigate; stop it from logging "not implemented".
    link.addEventListener('click', (e) => e.preventDefault())
    click(link)
    expect(opener.openUrl).not.toHaveBeenCalled()
    expect(offOrigin.openOffOriginHttp).not.toHaveBeenCalled()
    expect(outer).not.toHaveBeenCalled()
  })

  it('hosted web: a file: link is still refused', () => {
    web.on = true
    const { container } = render(<ChatMessageBody text={'[x](file:///etc/passwd)'} />)
    const link = onlyAnchor(container)
    expect(link.getAttribute('target')).toBeNull()
    const ev = click(link)
    expect(ev.defaultPrevented).toBe(true)
    expect(opener.openUrl).not.toHaveBeenCalled()
  })
})

describe('externalLinkUrl', () => {
  it('allows absolute http, https and mailto', () => {
    expect(externalLinkUrl('https://example.com')).toBe('https://example.com/')
    expect(externalLinkUrl('  HTTP://Example.com/a b ')).toBe('http://example.com/a%20b')
    expect(externalLinkUrl('mailto:a@example.com')).toBe('mailto:a@example.com')
  })

  it('refuses every other scheme and anything relative', () => {
    for (const href of [
      '',
      'javascript:alert(1)',
      ' JavaScript:alert(1)',
      'java\tscript:alert(1)',
      'data:text/html,<b>x</b>',
      'file:///etc/passwd',
      'vscode://file/x',
      'tel:+15555550100',
      'docs/a.md',
      '/cli/status',
      '//example.com/x',
      '#top',
      'https://',
    ]) {
      expect([href, externalLinkUrl(href)]).toEqual([href, null])
    }
  })
})

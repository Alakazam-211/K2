// @vitest-environment jsdom
// K2's own notices in an agent's Thread draw as system rows, so it is plain
// they come from K2 and not the agent:
//
//   1. A post stamped `author: {kind: "system"}` is a centred, full-width
//      row: no bubble, no picture, no speaker name; a small K2 mark and
//      "K2 · system"; the words without their leading "K2: ".
//   2. Its kind sets its tone: info is neutral, delivery wears the warning
//      tone with a warning icon (never the error red), action has buttons.
//   3. Its buttons come from `notice.actions`: a route posts its body to
//      the Thread's server; open_terminal opens the agent's terminal. A
//      post shows a busy state, then "Sent again" with a tick, or the
//      reason in muted words (and the button again).
//   4. A post without the stamp stays the agent's bubble, "K2: " and all.
//   5. The row takes focus, and its buttons follow it in the tab order.
import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async (..._args: unknown[]): Promise<unknown> => ({ ok: true })),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.daemonCliGet, daemonCliPost: h.daemonCliPost }))

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { fakeScope } from '@/test-utils/fake-scope'
import { ThreadAttachmentSourceProvider } from './ThreadImageObjects'
import { ThreadBubble } from './ThreadZenKit'
import { THREAD_SYSTEM_NOTICE_CSS, resetSystemNoticeDoneForTests } from './ThreadSystemNotice'
import { ACTION_NOTICE, DELIVERY_NOTICES, INFO_NOTICES, LOOKALIKE_AGENT_POST, OLD_DAEMON_NOTICE } from './systemNotice.fixtures'

const scope = fakeScope('box.example.k2.dev')

function chrome(openTerminal?: () => void): SessionViewChromeValue {
  return {
    viewTab: 'thread',
    splitLeft: 'terminal',
    splitRight: 'thread',
    overlayAddr: 'k2-design',
    conversationId: 'c1',
    agentName: 'k2-design',
    ...(openTerminal ? { openTerminal } : {}),
  }
}

function draw(item: OverlayThreadItem, opts: { openTerminal?: () => void; noSource?: boolean } = {}) {
  const bubble = <ThreadBubble item={item} mine={false} author="k2-design" timeLabel="2m" viewer={null} runStart tail />
  const inner: ReactNode = opts.noSource ? bubble : <ThreadAttachmentSourceProvider value={{ scope, addr: 'k2-design' }}>{bubble}</ThreadAttachmentSourceProvider>
  return render(<SessionViewChromeContext.Provider value={chrome(opts.openTerminal)}>{inner}</SessionViewChromeContext.Provider>)
}

const row = (): HTMLElement => screen.getByTestId('thread-system-notice')

afterEach(() => {
  cleanup()
  resetSystemNoticeDoneForTests()
  h.daemonCliPost.mockReset()
  h.daemonCliPost.mockImplementation(async () => ({ ok: true }))
})

describe('a K2 system notice in the Thread', () => {
  it('draws a row, not a bubble: no picture, no speaker name, the K2 label, the words without "K2: "', () => {
    const { container } = draw(INFO_NOTICES.jobRemoved)
    expect(container.querySelector('[data-zen-bubble]')).toBeNull()
    expect(container.querySelector('[data-thread-author]')).toBeNull()
    expect(container.querySelector('[data-thread-sender]')).toBeNull()
    expect(container.querySelector('[data-thread-meta]')).toBeNull()
    const r = row()
    expect(r.getAttribute('data-testid')).toBe('thread-system-notice')
    expect(r.closest('[data-zen-message]')).not.toBeNull()
    const label = r.querySelector('[data-notice-label]')!
    expect(label.textContent).toBe('K2 · system')
    expect(label.querySelector('[data-notice-mark]')).not.toBeNull()
    const words = r.querySelector('[data-notice-text]')!
    expect(words.textContent).toBe('job nightly-backup removed by ada.')
    expect(words.textContent).not.toContain('K2:')
    // Its Markdown is K2's own (a job's name in bold).
    expect(words.querySelector('strong')?.textContent).toBe('nightly-backup')
  })

  it('spans the column, centred', () => {
    draw(INFO_NOTICES.jobAdded)
    const outer = row().closest('[data-zen-message]') as HTMLElement
    expect(outer.style.alignSelf).toBe('stretch')
    expect(THREAD_SYSTEM_NOTICE_CSS).toMatch(/\[data-thread-system-notice\][^{]*\{[^}]*text-align: center/)
  })

  it('info is neutral: muted tone, no icon', () => {
    draw(INFO_NOTICES.testGuest)
    const r = row()
    expect(r.getAttribute('data-notice-kind')).toBe('info')
    expect(r.style.getPropertyValue('--notice-tone')).toBe('var(--zen-text-muted)')
    expect(r.querySelector('[data-notice-icon]')).toBeNull()
  })

  it('delivery wears the warning tone and a warning icon, never the error red', () => {
    draw(DELIVERY_NOTICES.retryFailed)
    const r = row()
    expect(r.getAttribute('data-notice-kind')).toBe('delivery')
    const tone = r.style.getPropertyValue('--notice-tone')
    expect(tone).toContain('--color-status-warn')
    expect(tone).not.toMatch(/error|danger/)
    expect(r.querySelector('[data-notice-icon="warn"]')).not.toBeNull()
    expect(THREAD_SYSTEM_NOTICE_CSS).not.toMatch(/status-error|zen-danger|--color-danger/)
  })

  it('action draws its buttons', () => {
    draw(ACTION_NOTICE)
    const r = row()
    expect(r.getAttribute('data-notice-kind')).toBe('action')
    expect(r.style.getPropertyValue('--notice-tone')).toContain('--zen-accent')
    expect(screen.getByRole('button', { name: 'Send again' })).toBeTruthy()
  })

  it('Send again posts the notice body to its route on the Thread\'s server, busy then "Sent again"', async () => {
    let finish!: (v: unknown) => void
    h.daemonCliPost.mockImplementation(() => new Promise((r) => (finish = r)))
    draw(DELIVERY_NOTICES.neverReadIn, { openTerminal: vi.fn() })
    fireEvent.click(screen.getByRole('button', { name: 'Send again' }))
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost).toHaveBeenCalledWith(scope, 'threads/resend', { thread: 'k2-design', doc: 'd-tokens' })
    const busy = screen.getByRole('button', { name: 'Sending…' }) as HTMLButtonElement
    expect(busy.disabled).toBe(true)
    expect(busy.getAttribute('aria-busy')).toBe('true')
    // A second click while busy sends nothing more.
    fireEvent.click(busy)
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    await act(async () => finish({ ok: true }))
    expect(screen.queryByRole('button', { name: /Send again|Sending/ })).toBeNull()
    const done = row().querySelector('[data-notice-done]')!
    expect(done.textContent).toBe('Sent again')
    expect(done.querySelector('[data-notice-tick]')).not.toBeNull()
    expect(done.getAttribute('role')).toBe('status')
    // The other button stays.
    expect(screen.getByRole('button', { name: 'Open its terminal' })).toBeTruthy()
    // Drawn again (scrolled away and back): still sent.
    cleanup()
    draw(DELIVERY_NOTICES.neverReadIn, { openTerminal: vi.fn() })
    expect(screen.queryByRole('button', { name: 'Send again' })).toBeNull()
    expect(row().querySelector('[data-notice-done]')?.textContent).toBe('Sent again')
  })

  it('a refused Send again shows the reason in muted words, and the button again', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw new Error('the message is gone')
    })
    draw(DELIVERY_NOTICES.neverReadIn)
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send again' }))
    })
    const why = row().querySelector('[data-notice-error]') as HTMLElement
    expect(why.textContent).toContain('the message is gone')
    expect(why.getAttribute('role')).toBe('alert')
    expect(why.style.color).toBe('var(--zen-text-muted)')
    expect((screen.getByRole('button', { name: 'Send again' }) as HTMLButtonElement).disabled).toBe(false)
    expect(row().querySelector('[data-notice-done]')).toBeNull()
  })

  it('Open its terminal opens the agent\'s terminal view, and posts nothing', () => {
    const openTerminal = vi.fn()
    draw(DELIVERY_NOTICES.neverReadIn, { openTerminal })
    fireEvent.click(screen.getByRole('button', { name: 'Open its terminal' }))
    expect(openTerminal).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('leaves out a button this view cannot carry out', () => {
    // No terminal here (a Garden, a view without one): no Open its terminal.
    draw(DELIVERY_NOTICES.neverReadIn)
    expect(screen.queryByRole('button', { name: 'Open its terminal' })).toBeNull()
    cleanup()
    // No server to post to: no Send again.
    draw(DELIVERY_NOTICES.neverReadIn, { noSource: true, openTerminal: vi.fn() })
    expect(screen.queryByRole('button', { name: 'Send again' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Open its terminal' })).toBeTruthy()
  })

  it('takes focus, with its buttons next in the tab order', () => {
    const { container } = draw(DELIVERY_NOTICES.neverReadIn, { openTerminal: vi.fn() })
    const r = row()
    expect(r.tabIndex).toBe(0)
    expect(r.getAttribute('role')).toBe('note')
    // Named by its label and words.
    const named = (r.getAttribute('aria-labelledby') ?? '').split(' ').map((id) => document.getElementById(id)?.textContent)
    expect(named[0]).toBe('K2 · system')
    expect(named[1]).toContain("didn't take the message")
    r.focus()
    expect(document.activeElement).toBe(r)
    const tabbable = Array.from(container.querySelectorAll<HTMLElement>('button, [tabindex]')).filter((el) => el.tabIndex >= 0 && !(el as HTMLButtonElement).disabled)
    expect(tabbable.map((el) => (el === r ? 'row' : el.textContent))).toEqual(['row', 'Send again', 'Open its terminal'])
    expect(tabbable.every((el) => el === r || el.tagName === 'BUTTON')).toBe(true)
    expect(THREAD_SYSTEM_NOTICE_CSS).toMatch(/\[data-thread-system-notice\]:focus-visible/)
  })
})

describe('not a system notice', () => {
  it('an agent post starting "K2: " with a notice field but no system stamp stays the agent\'s bubble', () => {
    const { container } = draw(LOOKALIKE_AGENT_POST, { openTerminal: vi.fn() })
    expect(screen.queryByTestId('thread-system-notice')).toBeNull()
    const bubble = container.querySelector('[data-zen-bubble="agent"]')!
    expect(bubble.textContent).toContain("K2: k2-design didn't take")
    expect(screen.queryByRole('button', { name: 'Send again' })).toBeNull()
  })

  it("an old daemon's notice (no stamp, no notice) stays the agent's bubble, words as sent", () => {
    const { container } = draw(OLD_DAEMON_NOTICE)
    expect(screen.queryByTestId('thread-system-notice')).toBeNull()
    expect(container.querySelector('[data-zen-bubble="agent"]')!.textContent).toContain('K2: k2-design')
  })
})

// @vitest-environment jsdom
// K2's own notices in an agent's Thread draw as system rows, so it is plain
// they come from K2 and not the agent:
//
//   1. A post stamped `author: {kind: "system"}` with a `systemNotice`, from
//      a server that reports `thread-system-notice-v1`, is a centred,
//      full-width row: no bubble, no picture, no speaker name; a small K2
//      mark and "K2 · system"; the words without their leading "K2: ".
//      Without the key, the same post stays a bubble.
//   2. Its kind sets its tone: info is neutral, delivery wears the warning
//      tone with a warning icon (never the error red), action has buttons.
//   3. Send again posts `{thread, doc}` to `/cli/thread/resend` on the
//      Thread's server: busy, then a quiet "Sent again" with a tick. A
//      notice whose action carries `doneAt` draws that line from the start
//      (a reload too), "by <name>" when someone else pressed it. Each 409
//      says why in plain muted words; another failure shows the reason and
//      the button again.
//   4. Open its terminal opens the terminal of the agent its `workspace`
//      names: this Thread's agent in this view, another one on the window's
//      server through the Agents page.
//   5. An App guest sees the row without its buttons.
//   6. A post without the stamp stays the agent's bubble, "K2: " and all.
//   7. The row takes focus, and its buttons follow it in the tab order.
import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async (..._args: unknown[]): Promise<unknown> => ({ ok: true })),
  openAgentTerminal: vi.fn(async (_addr: string) => true),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.daemonCliGet, daemonCliPost: h.daemonCliPost }))
vi.mock('@/lib/open-workspace-thread', () => ({ openAgentTerminal: (addr: string) => h.openAgentTerminal(addr) }))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import type { ServerScope } from '@/kessel/server-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { OWNER_VIEWER, type ThreadViewer } from '@/lib/thread-viewer'
import { fakeScope } from '@/test-utils/fake-scope'
import { ThreadAttachmentSourceProvider } from './ThreadImageObjects'
import { ThreadBubble } from './ThreadZenKit'
import { RESEND_REFUSAL_WORDS, TERMINAL_NOT_FOUND_WORDS, THREAD_SYSTEM_NOTICE_CSS } from './ThreadSystemNotice'
import {
  ACTION_NOTICE,
  DELIVERY_NOTICES,
  INFO_NOTICES,
  LOOKALIKE_AGENT_POST,
  OLD_DAEMON_NOTICE,
  OPEN_TERMINAL_ACTION,
  resendAction,
} from './systemNotice.fixtures'

const FEATURE = 'thread-system-notice-v1'

/** A server that reports `features` (the notice key by default). */
function scopeWith(features: string[] = [FEATURE], opts: { windowHost?: boolean; remoteRoom?: boolean } = {}): ServerScope {
  return {
    ...fakeScope('box.example.k2.dev'),
    serverSupports: (f: string) => features.includes(f),
    isWindowHost: () => opts.windowHost ?? false,
    ...(opts.remoteRoom ? { remoteRoom: true } : {}),
  } as ServerScope
}

const scope = scopeWith()

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

function draw(
  item: OverlayThreadItem,
  opts: { openTerminal?: () => void; noSource?: boolean; scope?: ServerScope; viewer?: ThreadViewer | null; addr?: string } = {},
) {
  const bubble = <ThreadBubble item={item} mine={false} author="k2-design" timeLabel="2m" viewer={opts.viewer ?? OWNER_VIEWER} runStart tail />
  const inner: ReactNode = opts.noSource ? (
    bubble
  ) : (
    <ThreadAttachmentSourceProvider value={{ scope: opts.scope ?? scope, addr: opts.addr ?? 'k2-design' }}>{bubble}</ThreadAttachmentSourceProvider>
  )
  return render(<SessionViewChromeContext.Provider value={chrome(opts.openTerminal)}>{inner}</SessionViewChromeContext.Provider>)
}

/** The never-read-in notice with its own `actions`. */
function delivery(actions: unknown[]): OverlayThreadItem {
  const n = DELIVERY_NOTICES.neverReadIn
  return { ...n, doc: { ...n.doc, systemNotice: { kind: 'delivery', code: 'delivery.not_taken.never_read_in', actions } } }
}

const row = (): HTMLElement => screen.getByTestId('thread-system-notice')

afterEach(() => {
  cleanup()
  h.daemonCliPost.mockReset()
  h.daemonCliPost.mockImplementation(async () => ({ ok: true }))
  h.openAgentTerminal.mockReset()
  h.openAgentTerminal.mockImplementation(async () => true)
})

describe('a K2 system notice in the Thread', () => {
  it('draws a row, not a bubble: no picture, no speaker name, the K2 label, the words without "K2: "', () => {
    const { container } = draw(INFO_NOTICES.jobRemoved)
    expect(container.querySelector('[data-zen-bubble]')).toBeNull()
    expect(container.querySelector('[data-thread-author]')).toBeNull()
    expect(container.querySelector('[data-thread-sender]')).toBeNull()
    expect(container.querySelector('[data-thread-meta]')).toBeNull()
    const r = row()
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

  it('hides the "[K2] " lead too', () => {
    draw(INFO_NOTICES.landed)
    const words = row().querySelector('[data-notice-text]')!.textContent ?? ''
    expect(words.startsWith('Landed 2 commits')).toBe(true)
    expect(words).not.toContain('[K2]')
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
    expect(r.getAttribute('data-notice-code')).toBe('app.test_guest_added')
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
})

describe('the feature key', () => {
  it('a server without thread-system-notice-v1 gets nothing special: the post stays a bubble', () => {
    const { container } = draw(DELIVERY_NOTICES.neverReadIn, { scope: scopeWith([]), openTerminal: vi.fn() })
    expect(screen.queryByTestId('thread-system-notice')).toBeNull()
    expect(container.querySelector('[data-zen-bubble]')).not.toBeNull()
    expect(screen.queryByRole('button', { name: 'Send again' })).toBeNull()
  })

  it('a view that knows no server gets nothing special either', () => {
    const { container } = draw(DELIVERY_NOTICES.neverReadIn, { noSource: true, openTerminal: vi.fn() })
    expect(screen.queryByTestId('thread-system-notice')).toBeNull()
    expect(container.querySelector('[data-zen-bubble]')).not.toBeNull()
  })
})

describe('Send again', () => {
  it('posts {thread, doc} to /cli/thread/resend on the Thread\'s server: busy, then "Sent again" with a tick', async () => {
    let finish!: (v: unknown) => void
    h.daemonCliPost.mockImplementation(() => new Promise((r) => (finish = r)))
    draw(DELIVERY_NOTICES.neverReadIn, { openTerminal: vi.fn() })
    fireEvent.click(screen.getByRole('button', { name: 'Send again' }))
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost).toHaveBeenCalledWith(scope, 'thread/resend', { thread: 'k2-design', doc: 'n-never' })
    const busy = screen.getByRole('button', { name: 'Sending…' }) as HTMLButtonElement
    expect(busy.disabled).toBe(true)
    expect(busy.getAttribute('aria-busy')).toBe('true')
    // A second click while busy sends nothing more.
    fireEvent.click(busy)
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    await act(async () => finish({ ok: true, resent: true, doneAt: 1_793_000_100 }))
    expect(screen.queryByRole('button', { name: /Send again|Sending/ })).toBeNull()
    const done = row().querySelector('[data-notice-done]')!
    expect(done.textContent).toBe('Sent again')
    expect(done.querySelector('[data-notice-tick]')).not.toBeNull()
    expect(done.getAttribute('role')).toBe('status')
    // The other button stays.
    expect(screen.getByRole('button', { name: 'Open its terminal' })).toBeTruthy()
  })

  it('draws done from the notice: doneAt alone is done, no button, on every draw', () => {
    const pressedByMe = delivery([resendAction('n-never', { doneAt: 1_793_000_100, doneBy: { kind: 'owner', login: 'owner' } }), OPEN_TERMINAL_ACTION])
    draw(pressedByMe, { openTerminal: vi.fn() })
    expect(screen.queryByRole('button', { name: 'Send again' })).toBeNull()
    expect(row().querySelector('[data-notice-done]')?.textContent).toBe('Sent again')
    // The open button is the first button now.
    expect(screen.getByRole('button', { name: 'Open its terminal' })).toBeTruthy()
    cleanup()
    // Without the field (the press before the reload left no trace here):
    // the button, as the daemon says.
    draw(DELIVERY_NOTICES.neverReadIn, { openTerminal: vi.fn() })
    expect(screen.getByRole('button', { name: 'Send again' })).toBeTruthy()
  })

  it('says who pressed it when that was someone else', () => {
    draw(delivery([resendAction('n-never', { doneAt: 1_793_000_100, doneBy: { kind: 'user', login: 'ada' } })]), { viewer: OWNER_VIEWER })
    expect(row().querySelector('[data-notice-done]')?.textContent).toBe('Sent again by ada')
    cleanup()
    // ada looking: her own press.
    draw(delivery([resendAction('n-never', { doneAt: 1_793_000_100, doneBy: { kind: 'user', login: 'ada' } })]), {
      viewer: { principal: 'ada', aliases: [] },
    })
    expect(row().querySelector('[data-notice-done]')?.textContent).toBe('Sent again')
    cleanup()
    // The owner pressed it, ada looking: the owner's stamped name.
    draw(delivery([resendAction('n-never', { doneAt: 1_793_000_100, doneBy: { kind: 'owner', login: 'owner', name: 'rosson' } })]), {
      viewer: { principal: 'ada', aliases: [] },
    })
    expect(row().querySelector('[data-notice-done]')?.textContent).toBe('Sent again by rosson')
  })

  it.each([
    ['already_resent', 'Already sent again.'],
    ['source_deleted', 'The original message was deleted.'],
    ['not_resendable', 'This one can’t be sent again.'],
  ])('a 409 %s says "%s" in plain muted words, in place of the button', async (code, words) => {
    expect(RESEND_REFUSAL_WORDS[code]).toBe(words)
    h.daemonCliPost.mockImplementation(async () => {
      throw new DaemonHttpError('refused', 409, code)
    })
    draw(DELIVERY_NOTICES.neverReadIn, { openTerminal: vi.fn() })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send again' }))
    })
    const said = row().querySelector('[data-notice-refused]') as HTMLElement
    expect(said.textContent).toBe(words)
    expect(said.style.color).toBe('var(--zen-text-muted)')
    expect(screen.queryByRole('button', { name: 'Send again' })).toBeNull()
    expect(row().querySelector('[data-notice-done]')).toBeNull()
    expect(row().querySelector('[data-notice-error]')).toBeNull()
  })

  it('another failure shows the reason in muted words, and the button again', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw new Error('the server is away')
    })
    draw(DELIVERY_NOTICES.neverReadIn)
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send again' }))
    })
    const why = row().querySelector('[data-notice-error]') as HTMLElement
    expect(why.textContent).toContain('the server is away')
    expect(why.getAttribute('role')).toBe('alert')
    expect(why.style.color).toBe('var(--zen-text-muted)')
    expect((screen.getByRole('button', { name: 'Send again' }) as HTMLButtonElement).disabled).toBe(false)
    expect(row().querySelector('[data-notice-done]')).toBeNull()
  })

  it('a room on another server may send it (the room write list has it)', () => {
    draw(DELIVERY_NOTICES.neverReadIn, { scope: scopeWith([FEATURE], { remoteRoom: true }) })
    expect(screen.getByRole('button', { name: 'Send again' })).toBeTruthy()
  })
})

describe('Open its terminal', () => {
  it("opens this Thread's agent in this view, and posts nothing", () => {
    const openTerminal = vi.fn()
    draw(DELIVERY_NOTICES.neverReadIn, { openTerminal })
    fireEvent.click(screen.getByRole('button', { name: 'Open its terminal' }))
    expect(openTerminal).toHaveBeenCalledTimes(1)
    expect(h.openAgentTerminal).not.toHaveBeenCalled()
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('opens the agent its workspace names, not this one, when that is another agent', async () => {
    const openTerminal = vi.fn()
    draw(delivery([{ ...OPEN_TERMINAL_ACTION, workspace: 'ledger' }]), { openTerminal, scope: scopeWith([FEATURE], { windowHost: true }) })
    fireEvent.click(screen.getByRole('button', { name: 'Open its terminal' }))
    await waitFor(() => expect(h.openAgentTerminal).toHaveBeenCalledWith('ledger'))
    expect(openTerminal).not.toHaveBeenCalled()
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('opens its own agent through the Agents page from a view with no terminal (a Home conversation)', async () => {
    draw(DELIVERY_NOTICES.neverReadIn, { scope: scopeWith([FEATURE], { windowHost: true }) })
    fireEvent.click(screen.getByRole('button', { name: 'Open its terminal' }))
    await waitFor(() => expect(h.openAgentTerminal).toHaveBeenCalledWith('k2-design'))
  })

  it('says so when the window\'s server has no such agent', async () => {
    h.openAgentTerminal.mockImplementation(async () => false)
    draw(delivery([{ ...OPEN_TERMINAL_ACTION, workspace: 'gone' }]), { scope: scopeWith([FEATURE], { windowHost: true }) })
    fireEvent.click(screen.getByRole('button', { name: 'Open its terminal' }))
    await waitFor(() => expect(row().querySelector('[data-notice-error]')?.textContent).toBe(TERMINAL_NOT_FOUND_WORDS))
  })

  it("isn't drawn where it can't open: another agent on another server, or no terminal and not the window's server", () => {
    draw(delivery([{ ...OPEN_TERMINAL_ACTION, workspace: 'ledger' }]), { openTerminal: vi.fn() })
    expect(screen.queryByRole('button', { name: 'Open its terminal' })).toBeNull()
    cleanup()
    draw(DELIVERY_NOTICES.neverReadIn)
    expect(screen.queryByRole('button', { name: 'Open its terminal' })).toBeNull()
  })
})

describe('an App guest', () => {
  it('sees the row and its words, but no buttons', () => {
    const guest: ThreadViewer = { principal: 'qa-1', aliases: [], guest: true }
    draw(DELIVERY_NOTICES.neverReadIn, { viewer: guest, openTerminal: vi.fn() })
    expect(row().querySelector('[data-notice-text]')?.textContent).toContain("didn't take")
    expect(screen.queryByRole('button')).toBeNull()
    expect(row().querySelector('[data-notice-actions]')).toBeNull()
  })
})

describe('focus', () => {
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
  it('an agent post starting "K2: " with a look-alike systemNotice but no system stamp stays the agent\'s bubble', () => {
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

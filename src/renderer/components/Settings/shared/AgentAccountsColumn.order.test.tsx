// @vitest-environment jsdom
//
// Settings → Subs / Tokens: the server default is always first in its
// provider's card, as on the phone (Rosson 2026-11-15: "so you don't have
// to go hunting for it"). The rest keep the daemon's order. Make default
// moves the row to the top as soon as the switch is sent (a short move on
// the Style's medium motion, none under reduced motion), keeps keyboard
// focus on that row and says so politely; a refusal puts the order back.
// With no default, the card's line says so above the list. Daemon calls
// are mocked; an unexpected route throws. Labels are samples.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: async () => {} }))

import { AgentAccountsColumn, defaultFirst } from './AgentAccountsColumn'
import { parseAccountsDoc, resetLlmAccountsForTests } from '@/stores/llm-accounts'

function account(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: 'acc_x', tool: 'claude', label: 'x', kind: 'subscription', active: false, state: 'signed_in', detail: null,
    email: null, org: null, plan: 'max', expiresAt: null, refreshedAt: null, lastUsedAt: null, createdAt: 1,
    createdBy: null, usage: null, usageCheckedAt: null, ...over,
  }
}

// The daemon's order: Studio, Team, Personal. Personal is the default.
const STUDIO = account({ id: 'acc_studio', label: 'Studio Max' })
const TEAM = account({ id: 'acc_team', label: 'Team Pro', plan: 'pro' })
const PERSONAL = account({ id: 'acc_personal', label: 'Personal Max' })

let defaultId: string | null = 'acc_personal'

function doc(): Record<string, unknown> {
  const notYet = (tool: string, display: string) => ({
    tool, display, supported: false, subscription: false, apiKeys: false, activeId: null, liveAccountId: null, loginMethod: null, accounts: [], pins: [],
  })
  return {
    tools: [
      {
        tool: 'claude', display: 'Claude', supported: true, subscription: true, apiKeys: true, activeId: defaultId,
        liveAccountId: defaultId, switchedAt: 1_700_000_000, switchedBy: 'owner-token', loginMethod: 'temp_home',
        accounts: [STUDIO, TEAM, PERSONAL].map((a) => ({ ...a, active: a.id === defaultId })), pins: [],
      },
      notYet('codex', 'Codex'), notYet('grok', 'Grok'), notYet('gemini', 'Gemini'),
      notYet('cursor', 'Cursor Agent'), notYet('pi', 'Pi'), notYet('hermes', 'Hermes'),
    ],
    logins: [],
    airgap: false,
    switchNote: 'x',
  }
}

/** The switch route: held until the test answers or refuses it. */
function holdSwitch(): { answer: () => void; fail: (msg: string) => void } {
  let answer = (): void => {}
  let fail = (_msg: string): void => {}
  h.daemonCliPost.mockImplementation((route: string, body: { id?: string }) => {
    if (route === 'llm/accounts/switch') {
      return new Promise((resolve, reject) => {
        answer = () => {
          defaultId = body.id ?? null
          resolve({ switch: { to: body.id, alreadyActive: false } })
        }
        fail = (msg) => reject(new Error(msg))
      })
    }
    if (route.startsWith('llm/accounts/')) return Promise.resolve({})
    return Promise.reject(new Error(`unexpected POST ${route}`))
  })
  return { answer: () => answer(), fail: (m) => fail(m) }
}

beforeEach(() => {
  resetLlmAccountsForTests()
  defaultId = 'acc_personal'
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/list') return doc()
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string) => {
    if (route.startsWith('llm/accounts/')) return {}
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

async function renderColumn(): Promise<void> {
  render(<AgentAccountsColumn />)
  await screen.findByTestId('llm-account-acc_personal')
}

/** The Claude card's rows, top to bottom. */
const order = (): string[] =>
  Array.from(screen.getByTestId('llm-tool-claude').querySelectorAll('[data-testid^="llm-account-"]')).map((el) =>
    (el.getAttribute('data-testid') ?? '').replace('llm-account-', ''),
  )

const makeDefault = (id: string): HTMLButtonElement =>
  within(screen.getByTestId(`llm-account-${id}`)).getByRole('button', { name: 'Make default' }) as HTMLButtonElement

const confirm = (): void => {
  fireEvent.click(within(screen.getByTestId('llm-switch-confirm')).getByRole('button', { name: 'Make default' }))
}

describe('the server default comes first', () => {
  it('on load, the default is first and the rest keep the daemon’s order', async () => {
    await renderColumn()
    expect(order()).toEqual(['acc_personal', 'acc_studio', 'acc_team'])
  })

  it('after Make default, the new default is first at once; the rest keep their order', async () => {
    const sw = holdSwitch()
    await renderColumn()
    fireEvent.click(makeDefault('acc_team'))
    confirm()
    await waitFor(() => expect(order()).toEqual(['acc_team', 'acc_studio', 'acc_personal']))
    sw.answer()
    await screen.findByText('“Team Pro” is now the Claude server default.')
    expect(order()).toEqual(['acc_team', 'acc_studio', 'acc_personal'])
  })

  it('a refused switch puts the order back', async () => {
    const sw = holdSwitch()
    await renderColumn()
    fireEvent.click(makeDefault('acc_team'))
    confirm()
    await waitFor(() => expect(order()[0]).toBe('acc_team'))
    sw.fail('the server is busy')
    expect((await screen.findByTestId('llm-row-error')).textContent).toContain('the server is busy')
    expect(order()).toEqual(['acc_personal', 'acc_studio', 'acc_team'])
  })

  it('keyboard focus stays on the row that moved, and a polite line says where it went', async () => {
    const sw = holdSwitch()
    await renderColumn()
    const live = screen.getByTestId('llm-order-announce')
    expect(live.getAttribute('aria-live')).toBe('polite')
    expect(live.textContent).toBe('')
    const btn = makeDefault('acc_team')
    btn.focus()
    fireEvent.click(btn)
    confirm()
    await waitFor(() => expect(order()[0]).toBe('acc_team'))
    await waitFor(() => expect(screen.getByTestId('llm-account-acc_team').contains(document.activeElement)).toBe(true))
    expect(document.activeElement).toBe(within(screen.getByTestId('llm-account-acc_team')).getByRole('button', { name: 'More actions for Team Pro' }))
    expect(live.textContent).toBe('“Team Pro” is now the Claude server default and is first in the list.')
    // A refusal moves it back; focus stays on the same row.
    sw.fail('the server is busy')
    await screen.findByTestId('llm-row-error')
    await waitFor(() => expect(order()[0]).toBe('acc_personal'))
    expect(screen.getByTestId('llm-account-acc_team').contains(document.activeElement)).toBe(true)
    expect(live.textContent).toBe('')
  })

  it('with no server default, the card says so above the list, and the order is the daemon’s', async () => {
    defaultId = null
    await renderColumn()
    const head = screen.getByTestId('llm-tool-claude')
    expect(head.textContent).toContain('No server default yet: chats set to Server default get no token from K2.')
    expect(order()).toEqual(['acc_studio', 'acc_team', 'acc_personal'])
    // The line sits above the first row.
    const line = within(head).getByText(/No server default yet/)
    const first = screen.getByTestId('llm-account-acc_studio')
    expect(line.compareDocumentPosition(first) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('defaultFirst: the default first, the rest in order; no default leaves the order', () => {
    const tool = parseAccountsDoc(doc()).tools[0]
    expect(defaultFirst(tool).map((a) => a.id)).toEqual(['acc_personal', 'acc_studio', 'acc_team'])
    defaultId = null
    const none = parseAccountsDoc(doc()).tools[0]
    expect(defaultFirst(none).map((a) => a.id)).toEqual(['acc_studio', 'acc_team', 'acc_personal'])
  })
})

describe('the move', () => {
  // jsdom has no layout: each row sits at 60px times its place in the card.
  let restoreTop: (() => void) | null = null
  beforeEach(() => {
    const was = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetTop')
    Object.defineProperty(HTMLElement.prototype, 'offsetTop', {
      configurable: true,
      get(this: HTMLElement) {
        return this.parentElement ? Array.from(this.parentElement.children).indexOf(this) * 60 : 0
      },
    })
    restoreTop = () => {
      if (was) Object.defineProperty(HTMLElement.prototype, 'offsetTop', was)
    }
  })
  afterEach(() => {
    restoreTop?.()
    delete (HTMLElement.prototype as Partial<HTMLElement>).animate
  })

  const reducedMotion = (on: boolean): void => {
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: on && q.includes('reduce'), media: q, addEventListener() {}, removeEventListener() {},
    }))
  }

  it('slides the rows to their new places on the Style’s medium motion', async () => {
    reducedMotion(false)
    const animate = vi.fn(() => ({ finished: Promise.resolve(), cancel() {} }))
    ;(HTMLElement.prototype as unknown as { animate: unknown }).animate = animate
    holdSwitch()
    await renderColumn()
    expect(animate).not.toHaveBeenCalled()
    fireEvent.click(makeDefault('acc_team'))
    confirm()
    await waitFor(() => expect(order()[0]).toBe('acc_team'))
    const moved = animate.mock.contexts.map((el) => (el as HTMLElement).getAttribute('data-testid'))
    expect(moved).toContain('llm-account-acc_team')
    const call = animate.mock.calls[moved.indexOf('llm-account-acc_team')] as unknown as [Keyframe[], KeyframeAnimationOptions]
    expect(call[0][0].transform).toMatch(/^translateY\(\d+px\)$/)
    expect(call[0][1].transform).toBe('translateY(0px)')
    expect(call[1].duration).toBeGreaterThan(0)
  })

  it('does not move under reduced motion', async () => {
    reducedMotion(true)
    const animate = vi.fn(() => ({ finished: Promise.resolve(), cancel() {} }))
    ;(HTMLElement.prototype as unknown as { animate: unknown }).animate = animate
    holdSwitch()
    await renderColumn()
    fireEvent.click(makeDefault('acc_team'))
    confirm()
    await waitFor(() => expect(order()[0]).toBe('acc_team'))
    expect(animate).not.toHaveBeenCalled()
  })
})

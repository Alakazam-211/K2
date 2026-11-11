// @vitest-environment jsdom
//
// Setup checklist component: draws the daemon's rows with the contract's
// copy, opens each row's step, skips and hides through the daemon, and
// draws nothing while the switch is off, or once hidden or finished.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  open: vi.fn((_opens: string | null) => true),
  handlers: new Map<string, Array<() => void>>(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

function sub(name: string) {
  return (_scope: unknown, fn: () => void) => {
    const list = h.handlers.get(name) ?? []
    list.push(fn)
    h.handlers.set(name, list)
    return () => {
      list.splice(list.indexOf(fn), 1)
    }
  }
}

vi.mock('@/stores/session-events', () => ({
  onFirstRunChanged: sub('first_run_changed'),
  onLlmAccountsChanged: sub('llm_accounts_changed'),
  onProjectsChanged: sub('projects_changed'),
  onTunnelStatusChanged: sub('tunnel_status_changed'),
  onTunnelIdentityChanged: sub('tunnel_identity_changed'),
  onPairCodeChanged: sub('pair_code_changed'),
  onAppHello: sub('hello'),
}))

vi.mock('@/lib/first-run-openers', () => ({ openFirstRunTarget: (o: string | null) => h.open(o) }))

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import {
  ALL_DONE_TEXT,
  CHECKLIST_ERROR_TEXT,
  NEEDS_K5_NOTE,
  NEEDS_TUNNEL_NOTE,
  SETUP_CHECKLIST_COPY,
  SET_UP_TEXT,
  SetupChecklist,
  SetupChecklistRailButton,
} from './SetupChecklist'
import { resetFirstRunForTests } from '@/stores/first-run'
import { useSidebarStore } from '@/stores/sidebar'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

type Body = { enabled: boolean; hidden: boolean; finished: boolean; rows: Array<Record<string, unknown>> }

function body(over: Partial<Body> = {}): Body {
  return {
    enabled: true,
    hidden: false,
    finished: false,
    rows: [
      { id: 'llm_sign_in', state: 'todo', opens: 'llm_sign_in' },
      { id: 'first_agent', state: 'done', opens: 'add_workspace' },
      { id: 'tunnel', state: 'todo', opens: 'tunnel' },
      { id: 'phone', state: 'todo', opens: 'tunnel', note: 'needs_tunnel' },
      { id: 'own_sign_in', state: 'todo', opens: 'owner_account_prompt' },
    ],
    ...over,
  }
}

async function settled(): Promise<void> {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

function checklist(): HTMLElement | null {
  return document.querySelector('[data-setup-checklist]')
}

beforeEach(() => {
  resetFirstRunForTests()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.open.mockClear()
  h.handlers.clear()
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'first-run/status') return body()
    throw new Error(`unexpected GET ${route}`)
  })
})

afterEach(() => cleanup())

describe('SetupChecklist', () => {
  it('marks the current step: the first row still to do, and only that one', async () => {
    h.daemonCliGet.mockResolvedValue(
      body({
        rows: [
          { id: 'llm_sign_in', state: 'done', opens: 'llm_sign_in' },
          { id: 'first_agent', state: 'skipped', opens: 'add_workspace' },
          { id: 'tunnel', state: 'todo', opens: 'tunnel' },
          { id: 'phone', state: 'todo', opens: 'tunnel', note: 'needs_tunnel' },
        ],
      }),
    )
    render(<SetupChecklist />)
    await screen.findByTestId('setup-row-tunnel')
    const current = [...document.querySelectorAll('[aria-current="step"]')].map((el) => el.getAttribute('data-testid'))
    expect(current).toEqual(['setup-row-tunnel'])
    expect(document.querySelector('[data-setup-row="tunnel"] [data-step-mark="todo"]')?.className).toContain('--color-accent')
    expect(document.querySelector('[data-setup-row="phone"] [data-step-mark="todo"]')?.className).not.toContain('--color-accent')
  })

  it('a skipped row reads through its mark and tag, not by fading', async () => {
    h.daemonCliGet.mockResolvedValue(body({ rows: [{ id: 'tunnel', state: 'skipped', opens: 'tunnel' }, { id: 'phone', state: 'todo', opens: 'tunnel' }] }))
    render(<SetupChecklist />)
    const row = await screen.findByTestId('setup-row-tunnel')
    expect(row.textContent).toContain('Skipped')
    expect(screen.getByTestId('setup-skipped-tunnel').textContent).toBe('Skipped')
    expect(document.querySelector('[data-setup-row="tunnel"]')!.innerHTML).not.toContain('opacity')
  })

  it("a row's note is no louder than its line", async () => {
    render(<SetupChecklist />)
    const row = await screen.findByTestId('setup-row-phone')
    const note = row.querySelector('[data-setup-note="needs_tunnel"]') as HTMLElement
    const line = [...row.querySelectorAll('span.type-hint')].find((el) => !el.hasAttribute('data-setup-note')) as HTMLElement
    const colour = (el: HTMLElement): string | undefined => el.className.match(/text-\[var\(--color-[a-z-]+\)\]/)?.[0]
    expect(colour(note)).toBe(colour(line))
  })

  it('draws each row with the contract copy and its state, in the daemon order', async () => {
    render(<SetupChecklist />)
    await screen.findByTestId('setup-row-llm_sign_in')
    const ids = [...document.querySelectorAll('[data-setup-row]')].map((el) => el.getAttribute('data-setup-row'))
    expect(ids).toEqual(['llm_sign_in', 'first_agent', 'tunnel', 'phone', 'own_sign_in'])
    expect(SETUP_CHECKLIST_COPY.llm_sign_in).toEqual({
      label: 'Sign in to Claude, Codex or Grok',
      line: 'Your agents think with this account.',
    })
    const first = screen.getByTestId('setup-row-llm_sign_in')
    expect(first.textContent).toContain('Sign in to Claude, Codex or Grok')
    expect(first.textContent).toContain('Your agents think with this account.')
    expect(first.textContent).toContain('To do')
    expect(screen.getByTestId('setup-row-first_agent').textContent).toContain('Done')
    expect(screen.getByTestId('setup-row-phone').textContent).toContain(NEEDS_TUNNEL_NOTE)
    expect(screen.getByTestId('setup-left').textContent).toBe('4 left')
    // A done row has nothing to skip.
    expect(screen.queryByTestId('setup-skip-first_agent')).toBeNull()
  })

  it.each([
    ['llm_sign_in', 'llm_sign_in'],
    ['first_agent', 'add_workspace'],
    ['tunnel', 'tunnel'],
    ['phone', 'tunnel'],
    ['own_sign_in', 'owner_account_prompt'],
  ])('the %s row runs its opener (%s)', async (id, opens) => {
    render(<SetupChecklist />)
    fireEvent.click(await screen.findByTestId(`setup-row-${id}`))
    expect(h.open).toHaveBeenCalledWith(opens)
  })

  it.each([
    ['off', { enabled: false }],
    ['hidden', { hidden: true }],
    ['finished', { finished: true }],
  ])('draws nothing when %s, in the sidebar or the rail', async (_name, over) => {
    h.daemonCliGet.mockResolvedValue(body(over))
    render(
      <>
        <SetupChecklist />
        <SetupChecklistRailButton />
      </>,
    )
    await settled()
    expect(h.daemonCliGet).toHaveBeenCalledWith('first-run/status')
    expect(checklist()).toBeNull()
    expect(screen.queryByTestId('setup-rail')).toBeNull()
  })

  it('draws nothing when the server has no checklist route', async () => {
    h.daemonCliGet.mockRejectedValue(new Error('route not found'))
    render(<SetupChecklist />)
    await settled()
    expect(checklist()).toBeNull()
  })

  it('Skip POSTs the row and draws the body the daemon returns', async () => {
    h.daemonCliPost.mockResolvedValue(
      body({ rows: body().rows.map((r) => (r.id === 'tunnel' ? { ...r, state: 'skipped' } : r)) }),
    )
    render(<SetupChecklist />)
    fireEvent.click(await screen.findByTestId('setup-skip-tunnel'))
    await waitFor(() => expect(screen.getByTestId('setup-row-tunnel').textContent).toContain('Skipped'))
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/skip', { row: 'tunnel' })
    expect(screen.queryByTestId('setup-skip-tunnel')).toBeNull()
    expect(screen.getByTestId('setup-left').textContent).toBe('3 left')
  })

  it('Hide POSTs and the checklist goes away', async () => {
    h.daemonCliPost.mockResolvedValue(body({ hidden: true }))
    render(<SetupChecklist />)
    fireEvent.click(await screen.findByTestId('setup-hide'))
    await waitFor(() => expect(checklist()).toBeNull())
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/hide', {})
  })

  it('shows again when first_run_changed says so', async () => {
    h.daemonCliGet.mockResolvedValue(body({ hidden: true }))
    render(<SetupChecklist />)
    await settled()
    expect(checklist()).toBeNull()
    h.daemonCliGet.mockResolvedValue(body())
    act(() => h.handlers.get('first_run_changed')?.forEach((fn) => fn()))
    expect(await screen.findByTestId('setup-row-tunnel')).toBeTruthy()
  })

  it('the K5 switch: chief_of_staff in first_agent\'s place, with its copy and the needs_k5 note', async () => {
    h.daemonCliGet.mockResolvedValue(
      body({
        rows: [
          { id: 'llm_sign_in', state: 'done', opens: 'llm_sign_in' },
          { id: 'chief_of_staff', state: 'todo', opens: 'k5_sheet', note: 'needs_k5' },
          { id: 'tunnel', state: 'todo', opens: 'tunnel' },
        ],
      }),
    )
    render(<SetupChecklist />)
    const row = await screen.findByTestId('setup-row-chief_of_staff')
    const ids = [...document.querySelectorAll('[data-setup-row]')].map((el) => el.getAttribute('data-setup-row'))
    expect(ids).toEqual(['llm_sign_in', 'chief_of_staff', 'tunnel'])
    expect(SETUP_CHECKLIST_COPY.chief_of_staff).toEqual({
      label: 'Hire your Chief of Staff',
      line: 'Your lead agent. K5 hires it with you.',
    })
    expect(NEEDS_K5_NOTE).toBe('You’ll meet K5 first.')
    expect(row.textContent).toContain('Hire your Chief of Staff')
    expect(row.textContent).toContain('Your lead agent. K5 hires it with you.')
    expect(row.querySelector('[data-setup-note="needs_k5"]')?.textContent).toBe(NEEDS_K5_NOTE)
    fireEvent.click(row)
    expect(h.open).toHaveBeenCalledWith('k5_sheet')
  })

  it('once K5 exists the row has no note and opens K5\'s Thread with the hire started', async () => {
    h.daemonCliGet.mockResolvedValue(
      body({ rows: [{ id: 'chief_of_staff', state: 'todo', opens: 'k5_thread_cos' }] }),
    )
    render(<SetupChecklist />)
    const row = await screen.findByTestId('setup-row-chief_of_staff')
    expect(row.querySelector('[data-setup-note]')).toBeNull()
    expect(row.textContent).not.toContain(NEEDS_K5_NOTE)
    fireEvent.click(row)
    expect(h.open).toHaveBeenCalledWith('k5_thread_cos')
  })

  it('a done chief_of_staff row drops its line and its note', async () => {
    h.daemonCliGet.mockResolvedValue(
      body({ rows: [{ id: 'chief_of_staff', state: 'done', opens: 'k5_thread_cos', note: 'needs_k5' }, { id: 'tunnel', state: 'todo', opens: 'tunnel' }] }),
    )
    render(<SetupChecklist />)
    const row = await screen.findByTestId('setup-row-chief_of_staff')
    expect(row.textContent).toContain('Hire your Chief of Staff')
    expect(row.textContent).not.toContain('K5 hires it with you.')
    expect(row.querySelector('[data-setup-note]')).toBeNull()
  })

  it('ignores row ids it does not know, and a row whose opens it does not know opens nothing', async () => {
    h.daemonCliGet.mockResolvedValue(
      body({
        rows: [
          { id: 'meet_k5', state: 'todo', opens: 'k5_chat' },
          { id: 'tunnel', state: 'todo', opens: 'something_new' },
          // An older daemon still sends the dropped k2 command row.
          { id: 'k2_cli', state: 'todo', opens: 'cli_settings' },
        ],
      }),
    )
    render(<SetupChecklist />)
    const row = await screen.findByTestId('setup-row-tunnel')
    expect(document.querySelectorAll('[data-setup-row]').length).toBe(1)
    expect((row as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(row)
    expect(h.open).not.toHaveBeenCalled()
  })
})

describe('SetupChecklist states', () => {
  afterEach(() => vi.useRealTimers())

  it('a failed gesture keeps the list, says so quietly, and Try again re-reads', async () => {
    h.daemonCliPost.mockRejectedValue(new Error('database is locked'))
    render(<SetupChecklist />)
    fireEvent.click(await screen.findByTestId('setup-skip-tunnel'))
    const alert = await screen.findByTestId('setup-error')
    expect(alert.textContent).toContain(CHECKLIST_ERROR_TEXT)
    expect(screen.getByTestId('setup-row-tunnel')).toBeTruthy()
    const reads = h.daemonCliGet.mock.calls.length
    fireEvent.click(screen.getByTestId('setup-retry'))
    await waitFor(() => expect(screen.queryByTestId('setup-error')).toBeNull())
    expect(h.daemonCliGet.mock.calls.length).toBe(reads + 1)
  })

  it('says the person is set up when the last row resolves, then draws nothing', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const oneLeft = body({ rows: body().rows.map((r) => ({ ...r, state: r.id === 'tunnel' ? 'todo' : 'done' })) })
    h.daemonCliGet.mockResolvedValue(oneLeft)
    h.daemonCliPost.mockResolvedValue({ ...oneLeft, finished: true })
    render(<SetupChecklist />)
    fireEvent.click(await screen.findByTestId('setup-skip-tunnel'))
    await waitFor(() => expect(document.querySelector('[data-setup-finished]')?.textContent).toContain(SET_UP_TEXT))
    act(() => {
      vi.advanceTimersByTime(4100)
    })
    expect(checklist()).toBeNull()
  })

  it('says all done in the header when every drawn row is resolved', async () => {
    h.daemonCliGet.mockResolvedValue(body({ rows: body().rows.map((r) => ({ ...r, state: 'done' })) }))
    render(<SetupChecklist />)
    await screen.findByTestId('setup-row-tunnel')
    expect(screen.getByTestId('setup-left').textContent).toBe(ALL_DONE_TEXT)
  })
})

describe('SetupChecklistRailButton', () => {
  it('shows how many rows are left and opens the nav', async () => {
    const expand = vi.fn()
    useSidebarStore.setState({ expand })
    render(<SetupChecklistRailButton />)
    const btn = await screen.findByTestId('setup-rail')
    expect(btn.textContent).toBe('4')
    expect(btn.querySelector('[data-k2-icon="checklist"]')).toBeTruthy()
    expect(screen.getByTestId('setup-rail-count').textContent).toBe('4')
    expect(btn.getAttribute('aria-label')).toBe('Setup checklist: 4 left')
    fireEvent.click(btn)
    expect(expand).toHaveBeenCalledTimes(1)
  })

  it('shows nothing when no row is left to do', async () => {
    h.daemonCliGet.mockResolvedValue(
      body({ rows: body().rows.map((r) => ({ ...r, state: r.id === 'tunnel' ? 'skipped' : 'done' })) }),
    )
    render(<SetupChecklistRailButton />)
    await settled()
    expect(screen.queryByTestId('setup-rail')).toBeNull()
  })
})

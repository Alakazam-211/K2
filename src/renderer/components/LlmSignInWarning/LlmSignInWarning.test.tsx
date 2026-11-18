// @vitest-environment jsdom
//
// The owners' pop-up before a Claude sign-in ends. Daemon calls are
// mocked; routes and bodies are asserted exactly. Fail loud: an
// unexpected route throws.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within, act } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  acctHandlers: [] as Array<(tool: string | null) => void>,
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

vi.mock('@/stores/session-events', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  onLlmAccountsChanged: (_scope: unknown, fn: (tool: string | null) => void) => {
    h.acctHandlers.push(fn)
    return () => {
      h.acctHandlers.splice(h.acctHandlers.indexOf(fn), 1)
    }
  },
}))

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: async () => {} }))

import { LlmSignInWarning, warningText, whenText } from './LlmSignInWarning'
import { resetLlmAccountsForTests } from '@/stores/llm-accounts'

const ENDS = 1_900_000_000
const SEVEN = { accountId: 'acc_jeff', tool: 'claude', label: 'JeffsToken', step: '7d', endsAt: ENDS, epoch: ENDS }

let served: unknown = { owner: true, warnings: [SEVEN] }

beforeEach(() => {
  resetLlmAccountsForTests()
  h.acctHandlers.length = 0
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  served = { owner: true, warnings: [SEVEN] }
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/warnings') return served
    if (route === 'llm/accounts/list') return { tools: [], logins: [], airgap: false, switchNote: '' }
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/warnings/dismiss') {
      served = { owner: true, warnings: [] }
      return { dismissed: {} }
    }
    if (route === 'llm/accounts/login') {
      return { account: { id: 'acc_jeff' }, login: { loginId: 'login_1', accountId: 'acc_jeff', tool: 'claude', label: 'JeffsToken', mode: 'other_device', method: 'temp_home', state: 'signing_in', url: null, code: null, error: null, screen: [], startedAt: 1, done: false, banner: null, offerMakeActive: false } }
    }
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => cleanup())

describe('the sign-in warning pop-up', () => {
  it('shows an owner the warning in plain words; Later puts this step away and it does not come back', async () => {
    render(<LlmSignInWarning />)
    const sheet = await screen.findByTestId('llm-sign-in-warning')
    expect(sheet.textContent).toContain(`Claude login JeffsToken needs a fresh sign-in by ${whenText(ENDS)}. After that, agents using it stop.`)
    fireEvent.click(within(sheet).getByRole('button', { name: 'Later' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/warnings/dismiss', { id: 'acc_jeff', step: '7d', epoch: ENDS }))
    expect(screen.queryByTestId('llm-sign-in-warning')).toBeNull()
    // The daemon's next change: read again, nothing to show.
    await act(async () => {
      for (const fn of [...h.acctHandlers]) fn('claude')
    })
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledTimes(2))
    expect(screen.queryByTestId('llm-sign-in-warning')).toBeNull()
  })

  it('Sign in now puts it away and starts the sign-in for that token', async () => {
    render(<LlmSignInWarning />)
    const sheet = await screen.findByTestId('llm-sign-in-warning')
    fireEvent.click(within(sheet).getByRole('button', { name: 'Sign in now' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/warnings/dismiss', { id: 'acc_jeff', step: '7d', epoch: ENDS }))
    expect(await screen.findByText('Sign “JeffsToken” in again')).toBeTruthy()
  })

  it('shows nothing to anyone who is not an owner', async () => {
    served = { owner: false, warnings: [SEVEN] }
    render(<LlmSignInWarning />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('llm/accounts/warnings'))
    await act(async () => {})
    expect(screen.queryByTestId('llm-sign-in-warning')).toBeNull()
  })

  it('appears when the daemon says a warning came due', async () => {
    served = { owner: true, warnings: [] }
    render(<LlmSignInWarning />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledTimes(1))
    expect(screen.queryByTestId('llm-sign-in-warning')).toBeNull()
    served = { owner: true, warnings: [{ ...SEVEN, step: 'ended' }] }
    await act(async () => {
      for (const fn of [...h.acctHandlers]) fn('claude')
    })
    const sheet = await screen.findByTestId('llm-sign-in-warning')
    expect(sheet.textContent).toContain('Claude login JeffsToken has ended')
  })

  it('says when an ended sign-in ended, and only when that is past', () => {
    const ended = { ...SEVEN, step: 'ended' }
    expect(warningText(ended, (ENDS + 60) * 1000).message).toBe(`Claude login JeffsToken has ended on ${whenText(ENDS)}. Agents using it stopped. Sign in again to start them.`)
    expect(warningText(ended, (ENDS - 60) * 1000).message).toBe('Claude login JeffsToken has ended. Agents using it stopped. Sign in again to start them.')
  })
})

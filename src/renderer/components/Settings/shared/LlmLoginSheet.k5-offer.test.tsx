// @vitest-environment jsdom
//
// K5 at first run: the sign-in sheet's success state shows the K5 offer only
// when the daemon says `offer: "meet_k5"`. Meet K5 opens the K5 sheet and
// closes this one; Not now posts the dismissal and the offer goes. A sign-in
// still under way, another offer value or another server shows nothing.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  addLogin: vi.fn(),
  loginStatus: vi.fn(),
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: async () => {} }))
vi.mock('@/stores/llm-accounts', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/stores/llm-accounts')>()
  return {
    ...real,
    addLogin: (...a: unknown[]) => h.addLogin(...a),
    loginStatus: (...a: unknown[]) => h.loginStatus(...a),
  }
})
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})
vi.mock('@/stores/session-events', () => {
  const off = () => () => {}
  return {
    onFirstRunChanged: off,
    onLlmAccountsChanged: off,
    onSubscriptionUsageChanged: off,
    onProjectsChanged: off,
    onTunnelStatusChanged: off,
    onTunnelIdentityChanged: off,
    onPairCodeChanged: off,
    onAppHello: off,
  }
})

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { act } from 'react'
import { LlmLoginSheet } from './LlmLoginSheet'
import { K5_OFFER_COPY } from '@/components/K5Sheet/K5Offer'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { resetFirstRunForTests } from '@/stores/first-run'
import { useK5SheetStore } from '@/stores/k5-sheet'

function login(state: string): Record<string, unknown> {
  return {
    loginId: 'login_1',
    accountId: 'acc_new',
    tool: 'claude',
    label: 'team',
    mode: 'this_computer',
    method: 'temp_home',
    state,
    url: null,
    code: null,
    error: null,
    screen: [],
    startedAt: 1,
    done: state === 'signed_in',
    banner: null,
    offerMakeActive: false,
  }
}

function status(offer: unknown): Record<string, unknown> {
  return { enabled: true, hidden: false, finished: false, offer, k5: null, k5Defaults: { name: 'K5', handle: 'k5', path: '/h/k2-agents/k5' }, rows: [] }
}

let offer: unknown = 'meet_k5'
const onClose = vi.fn()

beforeEach(() => {
  resetFirstRunForTests()
  useK5SheetStore.setState({ isOpen: false })
  onClose.mockReset()
  h.addLogin.mockReset()
  h.loginStatus.mockReset()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  offer = 'meet_k5'
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'first-run/status') return status(offer)
    throw new Error(`unexpected GET ${route}`)
  })
})

afterEach(() => cleanup())

async function signIn(state: string, scope: ServerScope = primaryScope()): Promise<void> {
  const l = login(state)
  h.addLogin.mockImplementation(async () => ({ account: { id: 'acc_new' }, login: l }))
  h.loginStatus.mockImplementation(async () => l)
  render(<LlmLoginSheet scope={scope} start={{ kind: 'add', tool: 'claude', toolLabel: 'Claude' }} onClose={onClose} />)
  fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'team' } })
  await act(async () => {
    fireEvent.click(screen.getByTestId('llm-login-start'))
  })
  await screen.findByTestId('llm-login-state')
}

describe('the K5 offer after a sign-in', () => {
  it('shows the offer for meet_k5 with the contract copy', async () => {
    await signIn('signed_in')
    await screen.findByTestId('k5-offer')
    expect(screen.getByText(K5_OFFER_COPY.title)).toBeTruthy()
    expect(screen.getByText(K5_OFFER_COPY.line)).toBeTruthy()
    expect(screen.getByTestId('k5-offer-meet').textContent).toBe('Meet K5')
    expect(screen.getByTestId('k5-offer-dismiss').textContent).toBe('Not now')
  })

  it('Meet K5 opens the K5 sheet and closes the sign-in sheet', async () => {
    await signIn('signed_in')
    fireEvent.click(await screen.findByTestId('k5-offer-meet'))
    expect(useK5SheetStore.getState().isOpen).toBe(true)
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('Not now posts the dismissal and the offer goes', async () => {
    await signIn('signed_in')
    h.daemonCliPost.mockResolvedValue(status(null))
    await act(async () => {
      fireEvent.click(await screen.findByTestId('k5-offer-dismiss'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/offer/dismiss', {})
    await waitFor(() => expect(screen.queryByTestId('k5-offer')).toBeNull())
    expect(useK5SheetStore.getState().isOpen).toBe(false)
  })

  it('no offer without meet_k5', async () => {
    offer = null
    await signIn('signed_in')
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('first-run/status'))
    await act(async () => {})
    expect(screen.queryByTestId('k5-offer')).toBeNull()
  })

  it('an unknown offer value shows nothing', async () => {
    offer = 'meet_k6'
    await signIn('signed_in')
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('first-run/status'))
    await act(async () => {})
    expect(screen.queryByTestId('k5-offer')).toBeNull()
  })

  it('no offer while the sign-in is still under way', async () => {
    await signIn('waiting_for_browser')
    await act(async () => {})
    expect(screen.queryByTestId('k5-offer')).toBeNull()
    expect(h.daemonCliGet).not.toHaveBeenCalled()
  })
})

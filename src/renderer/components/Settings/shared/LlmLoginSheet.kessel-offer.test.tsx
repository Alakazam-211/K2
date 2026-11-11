// @vitest-environment jsdom
//
// Kessel at first run: the sign-in sheet's success state shows the Kessel offer only
// when the daemon says `offer: "meet_kessel"`. Meet Kessel opens the Kessel sheet and
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
import { KESSEL_OFFER_COPY } from '@/components/KesselSheet/KesselOffer'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { resetFirstRunForTests } from '@/stores/first-run'
import { useKesselSheetStore } from '@/stores/kessel-sheet'

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
  return { enabled: true, hidden: false, finished: false, offer, kessel: null, kesselDefaults: { name: 'Kessel', handle: 'kessel', path: '/h/k2-agents/kessel' }, rows: [] }
}

let offer: unknown = 'meet_kessel'
const onClose = vi.fn()

beforeEach(() => {
  resetFirstRunForTests()
  useKesselSheetStore.setState({ isOpen: false })
  onClose.mockReset()
  h.addLogin.mockReset()
  h.loginStatus.mockReset()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  offer = 'meet_kessel'
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'first-run/status') return status(offer)
    throw new Error(`unexpected GET ${route}`)
  })
})

afterEach(() => cleanup())

async function signIn(state: string, scope: ServerScope = primaryScope(), over: Record<string, unknown> = {}): Promise<void> {
  const l = { ...login(state), ...over }
  h.addLogin.mockImplementation(async () => ({ account: { id: 'acc_new' }, login: l }))
  h.loginStatus.mockImplementation(async () => l)
  render(<LlmLoginSheet scope={scope} start={{ kind: 'add', tool: 'claude', toolLabel: 'Claude' }} onClose={onClose} />)
  fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'team' } })
  await act(async () => {
    fireEvent.click(screen.getByTestId('llm-login-start'))
  })
  // Signed in, the sheet shows its success note; under way, the state line.
  await screen.findByTestId(state === 'signed_in' ? 'llm-login-done' : 'llm-login-state')
}

describe('the Kessel offer after a sign-in', () => {
  it('shows the offer for meet_kessel with the contract copy', async () => {
    await signIn('signed_in')
    await screen.findByTestId('kessel-offer')
    expect(screen.getByText(KESSEL_OFFER_COPY.title)).toBeTruthy()
    expect(KESSEL_OFFER_COPY.title).toBe('Meet Kessel, K2\u2019s own agent')
    expect(KESSEL_OFFER_COPY.line.startsWith('Kessel helps you')).toBe(true)
    expect(screen.getByText(KESSEL_OFFER_COPY.line)).toBeTruthy()
    expect(screen.getByTestId('kessel-offer-meet').textContent).toBe('Meet Kessel')
    expect(screen.getByTestId('kessel-offer-dismiss').textContent).toBe('Not now')
  })

  it('sits under the success note, above the server-default question, with the only Not now', async () => {
    await signIn('signed_in', primaryScope(), { offerMakeActive: true })
    const offerBox = await screen.findByTestId('kessel-offer')
    const done = screen.getByTestId('llm-login-done')
    const question = screen.getByTestId('llm-login-offer')
    expect(done.compareDocumentPosition(offerBox) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(offerBox.compareDocumentPosition(question) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Not now' })).toHaveLength(1)
    expect(screen.getByRole('button', { name: 'Keep the current default' })).toBeTruthy()
  })

  it('Meet Kessel opens the Kessel sheet and closes the sign-in sheet', async () => {
    await signIn('signed_in')
    fireEvent.click(await screen.findByTestId('kessel-offer-meet'))
    expect(useKesselSheetStore.getState().isOpen).toBe(true)
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('Not now posts the dismissal and the offer goes', async () => {
    await signIn('signed_in')
    h.daemonCliPost.mockResolvedValue(status(null))
    await act(async () => {
      fireEvent.click(await screen.findByTestId('kessel-offer-dismiss'))
    })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/offer/dismiss', {})
    await waitFor(() => expect(screen.queryByTestId('kessel-offer')).toBeNull())
    expect(useKesselSheetStore.getState().isOpen).toBe(false)
  })

  it('the offer is a group named by its title, and waits while Not now is sent', async () => {
    await signIn('signed_in')
    const offerBox = await screen.findByTestId('kessel-offer')
    expect(screen.getByRole('group', { name: KESSEL_OFFER_COPY.title })).toBe(offerBox)
    let answer: (v: unknown) => void = () => {}
    h.daemonCliPost.mockImplementation(() => new Promise((r) => (answer = r)))
    await act(async () => {
      fireEvent.click(screen.getByTestId('kessel-offer-dismiss'))
    })
    expect((screen.getByTestId('kessel-offer-dismiss') as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTestId('kessel-offer-meet') as HTMLButtonElement).disabled).toBe(true)
    await act(async () => answer(status(null)))
    await waitFor(() => expect(screen.queryByTestId('kessel-offer')).toBeNull())
  })

  it('no offer without meet_kessel', async () => {
    offer = null
    await signIn('signed_in')
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('first-run/status'))
    await act(async () => {})
    expect(screen.queryByTestId('kessel-offer')).toBeNull()
  })

  it('an unknown offer value shows nothing', async () => {
    offer = 'meet_k6'
    await signIn('signed_in')
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('first-run/status'))
    await act(async () => {})
    expect(screen.queryByTestId('kessel-offer')).toBeNull()
  })

  it('no offer while the sign-in is still under way', async () => {
    await signIn('waiting_for_browser')
    await act(async () => {})
    expect(screen.queryByTestId('kessel-offer')).toBeNull()
    expect(h.daemonCliGet).not.toHaveBeenCalled()
  })
})

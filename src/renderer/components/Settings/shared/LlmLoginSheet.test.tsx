// @vitest-environment jsdom
//
// 0.46.3 S1: the sign-in link is never cut. The sheet shows the vendor host,
// a Show full link toggle and a Copy link button; Open and Copy use exactly
// the stored `login.url` (never text read back from the page). The device
// code has its own Copy button. Store calls are mocked; fail loud.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, act } from '@testing-library/react'

const h = vi.hoisted(() => ({
  openUrl: vi.fn(async (_url: string) => {}),
  addLogin: vi.fn(),
  loginStatus: vi.fn(),
  cancelLogin: vi.fn(),
}))

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (url: string) => h.openUrl(url) }))
vi.mock('@/stores/llm-accounts', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/stores/llm-accounts')>()
  return {
    ...real,
    addLogin: (...a: unknown[]) => h.addLogin(...a),
    loginAgain: () => {
      throw new Error('unexpected loginAgain')
    },
    loginStatus: (...a: unknown[]) => h.loginStatus(...a),
    cancelLogin: (...a: unknown[]) => h.cancelLogin(...a),
    sendLoginInput: () => {
      throw new Error('unexpected sendLoginInput')
    },
    switchLogin: () => {
      throw new Error('unexpected switchLogin')
    },
  }
})

import { LlmLoginSheet, signInHost } from './LlmLoginSheet'
import { primaryScope } from '@/kessel/server-scope'

/** Claude's real-shaped sign-in URL (~460 chars). */
const CLAUDE_URL =
  'https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload+user%3Aplugins&code_challenge=hhX09VfGxZ4Oe_6zAToJMg0tIdpFMzGge4PW-yYvPLo&code_challenge_method=S256&state=ciSJq3Xk2mV7pL0aB9cD4eF6gH8iJ1kL2mN3oP4qR5s'

/** A 2,000+ char URL: no length cap anywhere. */
const LONG_URL = `https://auth.openai.com/oauth/authorize?state=${'a1B2c3D4e5'.repeat(200)}`

function login(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    loginId: 'login_1',
    accountId: 'acc_new',
    tool: 'claude',
    label: 'team',
    mode: 'other_device',
    method: 'temp_home',
    state: 'waiting_for_code',
    url: CLAUDE_URL,
    urlHost: 'claude.ai',
    code: 'ABCD-EFGH',
    error: null,
    screen: [`If the browser didn't open, visit: ${CLAUDE_URL}`, 'Paste code here if prompted >'],
    startedAt: 1,
    done: false,
    banner: null,
    offerMakeActive: false,
    ...over,
  }
}

let writeText: ReturnType<typeof vi.fn>

beforeEach(() => {
  h.openUrl.mockClear()
  h.addLogin.mockReset()
  h.loginStatus.mockReset()
  h.cancelLogin.mockReset()
  h.loginStatus.mockImplementation(async () => {
    throw new Error('unexpected loginStatus (test did not set one)')
  })
  writeText = vi.fn(async (_t: string) => {})
  Object.assign(navigator, { clipboard: { writeText } })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

async function startSheet(l: Record<string, unknown>): Promise<void> {
  h.addLogin.mockImplementation(async () => ({ account: { id: 'acc_new' }, login: l }))
  // Polls return the same login so the sheet stays on it.
  h.loginStatus.mockImplementation(async () => l)
  render(
    <LlmLoginSheet
      scope={primaryScope()}
      start={{ kind: 'add', tool: String(l.tool), toolLabel: 'Claude' }}
      onClose={() => {}}
    />,
  )
  fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'team' } })
  fireEvent.click(screen.getByTestId('llm-login-start'))
  await screen.findByTestId('llm-login-link')
}

describe('LlmLoginSheet sign-in link (S1)', () => {
  it('shows "Sign in at <host>" with the full link hidden until toggled, then whole', async () => {
    await startSheet(login())
    expect(screen.getByTestId('llm-login-link').textContent).toContain('Sign in at claude.ai')
    expect(screen.getByTestId('llm-login-host').textContent).toBe('claude.ai')
    expect(screen.queryByTestId('llm-login-url')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Show full link' }))
    const shown = screen.getByTestId('llm-login-url')
    expect(shown.textContent).toBe(CLAUDE_URL)
    expect(shown.style.overflowWrap).toBe('anywhere')
    expect(shown.style.userSelect).toBe('all')
    expect(shown.className).not.toMatch(/truncate|ellipsis|line-clamp/)
    fireEvent.click(screen.getByRole('button', { name: 'Hide full link' }))
    expect(screen.queryByTestId('llm-login-url')).toBeNull()
  })

  it('Copy link writes exactly login.url, even when the page shows something else', async () => {
    await startSheet(login())
    fireEvent.click(screen.getByRole('button', { name: 'Show full link' }))
    // Tamper with what is on the page: Copy must not read it back.
    screen.getByTestId('llm-login-url').textContent = 'https://claude.ai/oauth/authorize?cut'
    fireEvent.click(screen.getByRole('button', { name: 'Copy link' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1))
    expect(writeText).toHaveBeenCalledWith(CLAUDE_URL)
    expect(await screen.findByRole('button', { name: 'Link copied' })).toBeTruthy()
  })

  it('Open sign-in page opens exactly login.url (a 2,000+ char URL, no cap)', async () => {
    await startSheet(login({ tool: 'codex', url: LONG_URL, urlHost: 'auth.openai.com', code: null }))
    expect(LONG_URL.length).toBeGreaterThan(2000)
    fireEvent.click(screen.getByRole('button', { name: 'Open sign-in page' }))
    expect(h.openUrl).toHaveBeenCalledTimes(1)
    expect(h.openUrl).toHaveBeenCalledWith(LONG_URL)
    fireEvent.click(screen.getByRole('button', { name: 'Copy link' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(LONG_URL))
    fireEvent.click(screen.getByRole('button', { name: 'Show full link' }))
    expect(screen.getByTestId('llm-login-url').textContent).toBe(LONG_URL)
  })

  it('the device code has its own Copy button, separate from Copy link', async () => {
    await startSheet(login({ tool: 'codex', url: 'https://auth.openai.com/codex/device', urlHost: 'auth.openai.com', code: 'ABCD-12345' }))
    expect(screen.getByTestId('llm-login-code').textContent).toBe('ABCD-12345')
    fireEvent.click(screen.getByRole('button', { name: 'Copy code' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('ABCD-12345'))
    expect(writeText).not.toHaveBeenCalledWith('https://auth.openai.com/codex/device')
    expect(screen.getByRole('button', { name: 'Copy link' })).toBeTruthy()
  })

  it('no link yet: no link row, the Open button waits', async () => {
    h.addLogin.mockImplementation(async () => ({ account: { id: 'acc_new' }, login: login({ url: null, urlHost: null, code: null, state: 'signing_in' }) }))
    h.loginStatus.mockImplementation(async () => login({ url: null, urlHost: null, code: null, state: 'signing_in' }))
    render(<LlmLoginSheet scope={primaryScope()} start={{ kind: 'add', tool: 'claude', toolLabel: 'Claude' }} onClose={() => {}} />)
    fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'team' } })
    fireEvent.click(screen.getByTestId('llm-login-start'))
    const wait = await screen.findByRole('button', { name: 'Waiting for the sign-in page…' })
    expect((wait as HTMLButtonElement).disabled).toBe(true)
    expect(screen.queryByTestId('llm-login-link')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Copy link' })).toBeNull()
  })

  it('the host falls back to the URL when an older daemon sends no urlHost', async () => {
    await startSheet(login({ urlHost: undefined }))
    expect(screen.getByTestId('llm-login-host').textContent).toBe('claude.ai')
    expect(signInHost({ url: 'not a url', urlHost: null })).toBeNull()
    expect(signInHost({ url: null, urlHost: null })).toBeNull()
  })

  it('Show terminal shows the logical lines, so the URL there is whole on one line', async () => {
    await startSheet(login())
    fireEvent.click(screen.getByRole('button', { name: 'Show terminal' }))
    const lines = (screen.getByTestId('llm-login-terminal').textContent ?? '').split('\n')
    expect(lines[0]).toBe(`If the browser didn't open, visit: ${CLAUDE_URL}`)
  })

  it('a re-issued link from polling replaces what Open and Copy use', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const next = `${CLAUDE_URL}&reissued=1`
    h.addLogin.mockImplementation(async () => ({ account: { id: 'acc_new' }, login: login() }))
    h.loginStatus.mockImplementation(async () => login({ url: next }))
    render(<LlmLoginSheet scope={primaryScope()} start={{ kind: 'add', tool: 'claude', toolLabel: 'Claude' }} onClose={() => {}} />)
    fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'team' } })
    fireEvent.click(screen.getByTestId('llm-login-start'))
    await screen.findByTestId('llm-login-link')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    await waitFor(() => expect(h.loginStatus).toHaveBeenCalled())
    fireEvent.click(screen.getByRole('button', { name: 'Open sign-in page' }))
    expect(h.openUrl).toHaveBeenLastCalledWith(next)
    fireEvent.click(screen.getByRole('button', { name: 'Copy link' }))
    await waitFor(() => expect(writeText).toHaveBeenLastCalledWith(next))
  })
})

// @vitest-environment jsdom
//
// The auto-rotate switch on Subs / Tokens. Daemon calls are mocked; routes
// and bodies are asserted exactly. Fail loud: an unexpected route throws.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

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

import { AutoNextRow, autoNextByText } from './AutoNextRow'
import { AUTO_NEXT_NOTE, parseAccountsDoc, resetLlmAccountsForTests, type LlmTool } from '@/stores/llm-accounts'
import { primaryScope } from '@/kessel/server-scope'

function tool(autoNext: unknown): LlmTool {
  const doc = parseAccountsDoc({ tools: [{ tool: 'claude', display: 'Claude', supported: true, subscription: true, accounts: [], autoNext }] })
  return doc.tools[0]
}

// `primaryOnly` checked the scope and forwards (path, body).
const posts = (): Array<[string, unknown]> => h.daemonCliPost.mock.calls.map((c) => [c[0] as string, c[1]])

beforeEach(() => {
  resetLlmAccountsForTests()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockImplementation(async (path: string) => {
    if (path === 'llm/accounts/list') return { tools: [] }
    throw new Error(`unexpected GET ${path}`)
  })
})

afterEach(() => cleanup())

describe('AutoNextRow', () => {
  it('is off when the daemon sends nothing (an older daemon) and says K2 never switches', () => {
    expect(tool(undefined).autoNext).toEqual({ on: false, by: null, lastLine: null, lastLineAt: null })
    render(<AutoNextRow scope={primaryScope()} tool={tool(undefined)} canChange changeReason={null} />)
    const sw = screen.getByRole('switch', { name: 'Switch Claude automatically at the limit' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(document.body.textContent).toContain('Off: K2 never switches on its own.')
  })

  it('asks first, says it may go against the provider’s terms, confirms it’s the person, then turns it on', async () => {
    h.daemonCliPost.mockImplementation(async (path: string) => {
      if (path === 'vault/confirm/start') return { methods: ['local-auth', 'vault-password'] }
      if (path === 'vault/confirm/finish') return { token: 'k2vc_test' }
      if (path === 'llm/accounts/auto-next/set') return { tools: [] }
      throw new Error(`unexpected POST ${path}`)
    })
    render(<AutoNextRow scope={primaryScope()} tool={tool({ on: false })} canChange changeReason={null} />)
    fireEvent.click(screen.getByRole('switch', { name: 'Switch Claude automatically at the limit' }))
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    const sheet = screen.getByTestId('llm-auto-next-confirm')
    expect(sheet.textContent).toContain(AUTO_NEXT_NOTE)
    expect(AUTO_NEXT_NOTE).toContain('may go against the provider’s terms')
    fireEvent.click(screen.getByRole('button', { name: 'Turn on' }))
    await waitFor(() => expect(screen.queryByTestId('llm-auto-next-confirm')).toBeNull())
    expect(posts()).toEqual([
      ['vault/confirm/start', { action: 'llm-auto-next', secretId: 'claude' }],
      ['vault/confirm/finish', { action: 'llm-auto-next', secretId: 'claude', method: 'local-auth', label: 'auto-rotate claude' }],
      ['llm/accounts/auto-next/set', { tool: 'claude', on: true, confirmToken: 'k2vc_test' }],
    ])
  })

  it('asks for the vault password when the server has no password-free confirm', async () => {
    h.daemonCliPost.mockImplementation(async (path: string) => {
      if (path === 'vault/confirm/start') return { methods: ['vault-password'] }
      if (path === 'vault/confirm/finish') return { token: 'k2vc_pw' }
      if (path === 'llm/accounts/auto-next/set') return { tools: [] }
      throw new Error(`unexpected POST ${path}`)
    })
    render(<AutoNextRow scope={primaryScope()} tool={tool({ on: false })} canChange changeReason={null} />)
    fireEvent.click(screen.getByRole('switch', { name: 'Switch Claude automatically at the limit' }))
    fireEvent.click(screen.getByRole('button', { name: 'Turn on' }))
    const field = await screen.findByLabelText('Vault password')
    fireEvent.change(field, { target: { value: 'pw-123' } })
    fireEvent.click(screen.getByRole('button', { name: 'Turn on' }))
    await waitFor(() => expect(screen.queryByTestId('llm-auto-next-confirm')).toBeNull())
    expect(posts().slice(-2)).toEqual([
      ['vault/confirm/finish', { action: 'llm-auto-next', secretId: 'claude', method: 'vault-password', label: 'auto-rotate claude', password: 'pw-123' }],
      ['llm/accounts/auto-next/set', { tool: 'claude', on: true, confirmToken: 'k2vc_pw' }],
    ])
  })

  it('turns off in one click and shows who turned it on and the last line', async () => {
    h.daemonCliPost.mockImplementation(async (path: string) => {
      if (path === 'llm/accounts/auto-next/set') return { tools: [] }
      throw new Error(`unexpected POST ${path}`)
    })
    const t = tool({ on: true, by: 'owner-token', lastLine: 'Switched to b automatically: a reached 99%', lastLineAt: 5 })
    render(<AutoNextRow scope={primaryScope()} tool={t} canChange changeReason={null} />)
    expect(document.body.textContent).toContain('turned on by the owner')
    expect(document.body.textContent).toContain('Switched to b automatically: a reached 99%')
    fireEvent.click(screen.getByRole('switch', { name: 'Switch Claude automatically at the limit' }))
    await waitFor(() => expect(posts()).toEqual([['llm/accounts/auto-next/set', { tool: 'claude', on: false }]]))
    expect(screen.queryByTestId('llm-auto-next-confirm')).toBeNull()
  })

  it('is disabled for someone who can’t change the server default', () => {
    render(<AutoNextRow scope={primaryScope()} tool={tool({ on: false })} canChange={false} changeReason="Only an Admin or the Owner can change the server default." />)
    const sw = screen.getByRole('switch', { name: 'Switch Claude automatically at the limit' })
    expect((sw as HTMLButtonElement).disabled).toBe(true)
    expect(autoNextByText('login:pat')).toBe('pat')
    expect(autoNextByText(null)).toBeNull()
  })
})

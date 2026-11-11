// @vitest-environment jsdom
// Settings → 3rd Party Bots → Grok Bot: what K2 found as a status row, the two
// switches with Saved / a failed save, a confirm before sending is turned on,
// plain errors with Try again, and a Member's read-only view.
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
const cli = vi.hoisted(() => ({
  settings: { show: false, send: false, canEdit: true } as Record<string, unknown>,
  detect: { installed: true, dataPath: '/Users/alex/sand-data', agents: [{ id: 'scout', name: 'Scout' }, { id: 'ledger', name: 'Ledger' }] } as unknown,
  getFail: null as Error | null,
  detectFail: null as Error | null,
  postFail: null as Error | null,
  posts: [] as unknown[],
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    if (route === 'grokbots/settings') {
      if (cli.getFail) throw cli.getFail
      return cli.settings
    }
    if (route === 'grokbots/detect') {
      if (cli.detectFail) throw cli.detectFail
      return cli.detect
    }
    if (route === 'grokbots/list') return { enabled: false, agents: [] }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body: Record<string, unknown>) => {
    if (route !== 'grokbots/settings') throw new Error(`unexpected POST ${route}`)
    cli.posts.push(body)
    if (cli.postFail) throw cli.postFail
    cli.settings = { ...cli.settings, ...body }
    return cli.settings
  }),
  isHostSwitchedError: () => false,
}))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { LocalGrokBotsSection } from './LocalGrokBotsSection'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => {
  cleanup()
  cli.settings = { show: false, send: false, canEdit: true }
  cli.detect = { installed: true, dataPath: '/Users/alex/sand-data', agents: [{ id: 'scout', name: 'Scout' }, { id: 'ledger', name: 'Ledger' }] }
  cli.getFail = null
  cli.detectFail = null
  cli.postFail = null
  cli.posts = []
  vi.restoreAllMocks()
})

describe('Settings → Grok Bot', () => {
  it('shows what K2 found as a status row, with the agents by name and the folder', async () => {
    render(<LocalGrokBotsSection />)
    const row = await screen.findByTestId('grokbots-found')
    expect(row.textContent).toBe('Grok Bot is on this computer2 agents: Scout and Ledger.Its data is in /Users/alex/sand-data.')
    expect((screen.getByRole('switch', { name: 'Let people message them' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByTestId('grokbots-send-needs-show').textContent).toBe('Turn on Show Grok Bot agents first.')
  })

  it('nothing found says where K2 looked, and Check again looks again', async () => {
    cli.detect = { installed: false, dataPath: '/Users/alex/sand-data', agents: [] }
    render(<LocalGrokBotsSection />)
    expect((await screen.findByTestId('grokbots-not-found')).textContent).toContain('K2 looked in /Users/alex/sand-data and found nothing')
    cli.detect = { installed: true, dataPath: '/Users/alex/sand-data', agents: [] }
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
    expect((await screen.findByTestId('grokbots-found')).textContent).toContain('No agents yet.')
  })

  it('a failed look is a plain error with Try again', async () => {
    cli.detectFail = new Error('permission denied')
    render(<LocalGrokBotsSection />)
    const card = await screen.findByTestId('grokbots-find-error')
    expect(card.textContent).toContain('Couldn’t check for Grok Bot')
    expect(card.textContent).toContain('The server said: permission denied.')
    cli.detectFail = null
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(await screen.findByTestId('grokbots-found')).toBeTruthy()
  })

  it('settings that fail to load show one error card, and Try again loads the page', async () => {
    cli.getFail = new Error('external_agents.json is unreadable')
    render(<LocalGrokBotsSection />)
    const card = await screen.findByTestId('grokbots-load-error')
    expect(card.textContent).toContain('Couldn’t load the Grok Bot settings')
    expect(screen.queryByRole('switch')).toBeNull()
    cli.getFail = null
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(await screen.findByRole('switch', { name: 'Show Grok Bot agents' })).toBeTruthy()
  })

  it('Show saves and says Saved; a refusal says why and leaves the switch off', async () => {
    render(<LocalGrokBotsSection />)
    const show = await screen.findByRole('switch', { name: 'Show Grok Bot agents' })
    fireEvent.click(show)
    expect((await screen.findByRole('status')).textContent).toBe('Saved')
    expect(cli.posts).toEqual([{ show: true }])
    expect(show.getAttribute('aria-checked')).toBe('true')
    cleanup()
    cli.settings = { show: false, send: false, canEdit: true }
    cli.postFail = new Error('only the owner or an admin may change this')
    render(<LocalGrokBotsSection />)
    const again = await screen.findByRole('switch', { name: 'Show Grok Bot agents' })
    fireEvent.click(again)
    expect((await screen.findByRole('alert')).textContent).toBe(
      'Couldn’t save: only the owner or an admin may change this. Nothing changed; try again.',
    )
    expect(again.getAttribute('aria-checked')).toBe('false')
  })

  it('turning sending on asks first (billed turns); cancelling sends nothing', async () => {
    cli.settings = { show: true, send: false, canEdit: true }
    const confirm = vi.spyOn(useConfirmDialogStore.getState(), 'confirm').mockResolvedValue(false)
    render(<LocalGrokBotsSection />)
    const send = await screen.findByRole('switch', { name: 'Let people message them' })
    fireEvent.click(send)
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(confirm.mock.calls[0]![0]).toMatchObject({ title: 'Let people message Grok Bot agents?', confirmLabel: 'Turn on sending' })
    expect(cli.posts).toEqual([])
    confirm.mockResolvedValue(true)
    fireEvent.click(send)
    await waitFor(() => expect(cli.posts).toEqual([{ send: true }]))
    await waitFor(() => expect(send.getAttribute('aria-checked')).toBe('true'))
  })

  it('a Member sees plain On / Off, no detection, and who can change it', async () => {
    cli.settings = { show: true, send: false, canEdit: false }
    render(<LocalGrokBotsSection />)
    expect((await screen.findByTestId('grokbots-settings-readonly')).textContent).toBe(
      'Only the server’s owner or an admin can change these.',
    )
    expect(screen.getByText('View only')).toBeTruthy()
    expect(screen.queryByRole('switch')).toBeNull()
    expect(screen.getByLabelText('Show Grok Bot agents').textContent).toBe('On')
    expect(screen.getByLabelText('Let people message them').textContent).toBe('Off')
    expect(screen.getByTestId('grokbots-member-status').textContent).toContain('Grok Bot agents show on this server')
    expect(screen.queryByTestId('grokbots-looking')).toBeNull()
  })
})

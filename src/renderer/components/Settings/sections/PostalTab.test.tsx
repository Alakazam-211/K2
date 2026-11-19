// @vitest-environment jsdom
// The workspace Postal tab (Beta) and the server's Postal switch
// (prd-postal-in-k2-v1 §5.5, vs-live P34):
//   - the tab shows only while the server lists `postal` in /boot-status
//     features, and never for a Voice & Chat only login;
//   - it shows the address card (with a claim form when there is no
//     address), Friends, Invitations and Routing;
//   - Accept posts postal/decide {workspace, id, decision: 'accept'}; on a
//     request with a pairing code it opens K2's code form, which shows this
//     server's code and sends the one typed from the other side;
//   - a 409 postal_off answer shows the daemon's hint inline;
//   - the owner's switch posts postal/settings {enabled} and re-reads
//     /boot-status so the tab appears.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

const cli = vi.hoisted(() => ({
  role: 'owner' as string,
  status: null as unknown,
  friends: [] as unknown[],
  settings: { enabled: false, adminsMayAccept: false },
  code: '' as string,
  getFail: null as Error | null,
  gets: [] as { route: string; params: unknown }[],
  posts: [] as { route: string; body: unknown }[],
  inviteResult: { ok: true } as unknown,
}))
const feature = vi.hoisted(() => ({ on: true }))
const noted = vi.hoisted(() => ({ calls: [] as unknown[][] }))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params: unknown) => {
    cli.gets.push({ route, params })
    if (route === 'auth/whoami') return { role: cli.role }
    if (cli.getFail && route.startsWith('postal/')) throw cli.getFail
    if (route === 'postal/status') return cli.status
    if (route === 'postal/friends') return { ok: true, state: 'on', handle: null, friends: cli.friends }
    if (route === 'postal/settings') return cli.settings
    if (route === 'postal/code') return { ok: true, code: cli.code }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body: unknown) => {
    cli.posts.push({ route, body })
    if (route === 'postal/settings') {
      cli.settings = { ...cli.settings, ...(body as object) }
      return { ok: true, ...cli.settings }
    }
    if (route === 'postal/invite') return cli.inviteResult
    return { ok: true }
  }),
  isHostSwitchedError: () => false,
}))

const fakeScope = vi.hoisted(() => ({
  id: 'primary',
  hostKey: 'local',
  isRemote: false,
  creds: async () => ({ host: '127.0.0.1', port: 1, token: 'tok' }),
  httpBase: async () => 'http://127.0.0.1:1',
}))

vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => fakeScope,
  reportedSupports: (_host: string, f: string) => f === 'postal' && feature.on,
  noteServerVersion: (...args: unknown[]) => {
    noted.calls.push(args)
  },
}))

vi.mock('@/stores/connect-host', async () => {
  const { create } = await import('zustand')
  return { useConnectHostStore: create(() => ({ activeHost: 'local', serverVersion: '0.46.14' })) }
})

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { PostalTab, POSTAL_ROUTING_LINE } from './PostalTab'
import { PostalServerSwitch } from './PostalServerSwitch'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const project = { id: 'ada', name: 'Ada', path: '/ws/ada' }
const scope = fakeScope as unknown as ServerScope

function statusBody(handle: unknown = null): unknown {
  return {
    ok: true,
    state: 'on',
    workspace: { id: 'ada', handle: 'ada' },
    handle,
    counts: { accepted: 0, pendingIn: 1, pendingOut: 0 },
    plane: { connected: false, note: 'The Postal network connection comes in a later release.' },
  }
}

beforeEach(() => {
  cli.role = 'owner'
  cli.status = statusBody()
  cli.friends = []
  cli.settings = { enabled: false, adminsMayAccept: false }
  cli.code = ''
  cli.getFail = null
  cli.gets = []
  cli.posts = []
  cli.inviteResult = { ok: true }
  feature.on = true
  noted.calls = []
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('PostalTab', () => {
  it('is hidden while the server does not list the postal feature', async () => {
    feature.on = false
    const { container } = render(<PostalTab project={project} scope={scope} />)
    await act(async () => {})
    expect(container.innerHTML).toBe('')
    expect(cli.gets.some((g) => g.route.startsWith('postal/'))).toBe(false)
  })

  it('is hidden for a Voice & Chat only login', async () => {
    cli.role = 'voice_chat'
    render(<PostalTab project={project} scope={scope} />)
    await waitFor(() => expect(screen.queryByTestId('postal-tab')).toBeNull())
  })

  it('shows the address card with a claim form, the note, Friends, Invitations and Routing', async () => {
    render(<PostalTab project={project} scope={scope} />)
    expect(await screen.findByTestId('postal-claim-form')).toBeTruthy()
    expect(screen.getByText('Claim a Postal address')).toBeTruthy()
    expect(screen.getByTestId('postal-plane-note').textContent).toBe('The Postal network connection comes in a later release.')
    expect(screen.getByText('Friends')).toBeTruthy()
    expect(screen.getByText('Invitations')).toBeTruthy()
    expect(screen.getByText('Routing')).toBeTruthy()
    expect(screen.getByTestId('postal-routing').textContent).toBe(POSTAL_ROUTING_LINE)
    expect(cli.gets.find((g) => g.route === 'postal/status')?.params).toEqual({ workspace: '/ws/ada' })
    expect(cli.gets.find((g) => g.route === 'postal/friends')?.params).toEqual({ workspace: '/ws/ada' })
  })

  it('claims the typed address', async () => {
    render(<PostalTab project={project} scope={scope} />)
    const field = await screen.findByLabelText('Postal address to claim')
    fireEvent.change(field, { target: { value: 'ada::acme.postal.bot' } })
    await act(async () => {
      fireEvent.submit(screen.getByTestId('postal-claim-form'))
    })
    expect(cli.posts).toContainEqual({ route: 'postal/claim', body: { workspace: '/ws/ada', address: 'ada::acme.postal.bot' } })
  })

  it('shows the address and no claim form once one is claimed', async () => {
    cli.status = statusBody({ id: 'h1', address: 'ada::acme.postal.bot', status: 'active', createdAt: '2026-11-17T00:00:00Z' })
    render(<PostalTab project={project} scope={scope} />)
    expect((await screen.findByTestId('postal-address')).textContent).toBe('ada::acme.postal.bot')
    expect(screen.queryByTestId('postal-claim-form')).toBeNull()
  })

  it('Accept posts postal/decide with the request id', async () => {
    cli.friends = [
      { id: 'f1', peer: 'scout::acme.postal.bot', status: 'pending_in', ownerName: 'Acme' },
      { id: 'f2', peer: 'bee::hive.postal.bot', status: 'accepted' },
      { id: 'f3', peer: 'old::x.postal.bot', status: 'rejected' },
    ]
    render(<PostalTab project={project} scope={scope} />)
    const accept = await screen.findByRole('button', { name: 'Accept' })
    // The accepted friend lists under Friends with Remove and Block.
    expect(screen.getByRole('button', { name: 'Remove' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Block' })).toBeTruthy()
    expect(screen.getByTestId('postal-history')).toBeTruthy()
    await act(async () => {
      fireEvent.click(accept)
    })
    expect(cli.posts).toContainEqual({ route: 'postal/decide', body: { workspace: '/ws/ada', id: 'f1', decision: 'accept' } })
  })

  it("a request with a pairing code: Accept opens K2's code form, which sends the other side's code", async () => {
    cli.friends = [{ id: 'f9', peer: 'lab::gamma.postal.bot', status: 'pending_in', ownerName: 'Dana', codeWaiting: true }]
    cli.code = '123 456'
    render(<PostalTab project={project} scope={scope} />)
    const accept = await screen.findByRole('button', { name: 'Accept' })
    await act(async () => {
      fireEvent.click(accept)
    })
    expect((await screen.findByTestId('postal-code-mine')).textContent).toBe('123 456')
    fireEvent.change(screen.getByTestId('postal-code-input'), { target: { value: '654 321' } })
    await act(async () => {
      fireEvent.submit(screen.getByTestId('postal-code-form'))
    })
    expect(cli.posts).toContainEqual({ route: 'postal/decide', body: { workspace: '/ws/ada', id: 'f9', decision: 'accept', code: '654321' } })
  })

  it('a request with a pairing code can still be rejected without a code', async () => {
    cli.friends = [{ id: 'f9', peer: 'lab::gamma.postal.bot', status: 'pending_in', ownerName: 'Dana', codeWaiting: true }]
    render(<PostalTab project={project} scope={scope} />)
    await screen.findByRole('button', { name: 'Accept' })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Reject' }))
    })
    expect(cli.posts).toContainEqual({ route: 'postal/decide', body: { workspace: '/ws/ada', id: 'f9', decision: 'reject' } })
  })

  it('lists blocked peers in their own Blocked group, not under Friends, with Unblock', async () => {
    cli.friends = [
      { id: 'f2', peer: 'bee::hive.postal.bot', status: 'accepted' },
      { id: 'f4', peer: 'pest::spam.postal.bot', status: 'blocked' },
    ]
    render(<PostalTab project={project} scope={scope} />)
    const blocked = await screen.findByTestId('postal-blocked')
    const rows = blocked.querySelectorAll('[data-testid="postal-peer-row"]')
    expect(Array.from(rows).map((r) => r.getAttribute('data-peer'))).toEqual(['pest::spam.postal.bot'])
    // Friends lists only the accepted one, with Remove; Unblock lives under Blocked.
    expect(screen.getAllByRole('button', { name: 'Remove' })).toHaveLength(1)
    expect(screen.getAllByRole('button', { name: 'Block' })).toHaveLength(1)
    const friendsRows = Array.from(document.querySelectorAll('[data-testid="postal-peer-row"]')).filter((r) => !blocked.contains(r))
    expect(friendsRows.map((r) => r.getAttribute('data-status'))).toEqual(['accepted'])
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Unblock' }))
    })
    expect(cli.posts).toContainEqual({ route: 'postal/remove', body: { workspace: '/ws/ada', peer: 'pest::spam.postal.bot' } })
    expect(screen.getByTestId('postal-notice').textContent).toBe('Unblocked pest::spam.postal.bot. It can send friend requests again.')
  })

  it('shows no Blocked group when nobody is blocked; Remove on a friend says Removed', async () => {
    cli.friends = [{ id: 'f2', peer: 'bee::hive.postal.bot', status: 'accepted' }]
    render(<PostalTab project={project} scope={scope} />)
    const remove = await screen.findByRole('button', { name: 'Remove' })
    expect(screen.queryByTestId('postal-blocked')).toBeNull()
    await act(async () => {
      fireEvent.click(remove)
    })
    expect(screen.getByTestId('postal-notice').textContent).toBe('Removed bee::hive.postal.bot.')
  })

  async function invite(to: string): Promise<void> {
    cli.status = statusBody({ id: 'h1', address: 'ada::acme.postal.bot', status: 'claimed', createdAt: '2026-11-17T00:00:00Z' })
    render(<PostalTab project={project} scope={scope} />)
    const field = await screen.findByLabelText('Address to invite')
    await waitFor(() => expect((field as HTMLInputElement).disabled).toBe(false))
    fireEvent.change(field, { target: { value: to } })
    await act(async () => {
      fireEvent.submit(screen.getByTestId('postal-invite-form'))
    })
  }

  it('the invite notice says sent when the daemon sent it', async () => {
    cli.inviteResult = { ok: true, sent: true, network: 'sent' }
    await invite('bee::hive.postal.bot')
    expect(cli.posts).toContainEqual({ route: 'postal/invite', body: { workspace: '/ws/ada', to: 'bee::hive.postal.bot' } })
    expect(screen.getByTestId('postal-notice').textContent).toBe('Invitation sent to bee::hive.postal.bot.')
  })

  it('the invite notice says it goes out later when the daemon could not send it yet', async () => {
    cli.inviteResult = { ok: true, sent: false, network: 'not_yet' }
    await invite('bee::hive.postal.bot')
    expect(screen.getByTestId('postal-notice').textContent).toBe(
      'Invitation to bee::hive.postal.bot goes out once this server connects to the Postal network.',
    )
  })

  it("shows the daemon's hint when Postal was turned off meanwhile", async () => {
    cli.getFail = new DaemonHttpError('Postal is off on this server. The owner turns it on in Settings.', 409, 'postal_off')
    render(<PostalTab project={project} scope={scope} />)
    const card = await screen.findByTestId('postal-status-error')
    expect(card.textContent).toContain('Postal is off on this server')
    expect(card.textContent).not.toMatch(/at \w+ \(/)
  })
})

describe('PostalServerSwitch', () => {
  it("the owner's flip posts enabled and re-reads /boot-status", async () => {
    const fetchMock = vi.fn(async () => new Response(JSON.stringify({ version: '0.46.14', features: ['postal'] }), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    render(<PostalServerSwitch scope={scope} />)
    const toggle = await screen.findByTestId('postal-switch')
    await waitFor(() => expect((toggle as HTMLButtonElement).disabled).toBe(false))
    await act(async () => {
      fireEvent.click(toggle)
    })
    expect(cli.posts).toContainEqual({ route: 'postal/settings', body: { enabled: true } })
    await waitFor(() => expect(noted.calls).toContainEqual(['local', '0.46.14', ['postal']]))
    expect(String((fetchMock.mock.calls[0] as unknown[])[0])).toBe('http://127.0.0.1:1/boot-status')
  })

  it('the hint says what the server sends over the Postal network, not that nothing leaves', async () => {
    render(<PostalServerSwitch scope={scope} />)
    const group = await screen.findByTestId('postal-server-switch')
    const text = group.textContent ?? ''
    expect(text).toContain('On: each workspace gets a Postal tab')
    expect(text).toContain(
      'This server connects to the Postal network to send and receive friend requests and your agents’ messages with their friends; anything sent while it isn’t connected waits until it is.',
    )
    expect(text).not.toMatch(/nothing\s+leaves/)
    expect(text).not.toMatch(/later release/)
  })

  it('a non-owner sees it read-only', async () => {
    cli.role = 'admin'
    render(<PostalServerSwitch scope={scope} />)
    const toggle = await screen.findByTestId('postal-switch')
    await screen.findByText(/Only this server.s owner can turn this on or off/)
    expect((toggle as HTMLButtonElement).disabled).toBe(true)
  })
})

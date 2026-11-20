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
//   - Remove and Block ask first; what happened is said in the card where
//     it happened; a request that stopped waiting reloads and says so;
//   - an invite the network hasn't sent yet says Not sent yet;
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
  postFail: {} as Record<string, Error>,
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
    if (cli.postFail[route]) throw cli.postFail[route]
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

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { PostalTab, POSTAL_ROUTING_LINE } from './PostalTab'
import { PostalServerSwitch } from './PostalServerSwitch'
import { postalRouteText } from './postal-api'
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
  cli.postFail = {}
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
    fireEvent.change(field, { target: { value: 'ada::initech.postal.bot' } })
    await act(async () => {
      fireEvent.submit(screen.getByTestId('postal-claim-form'))
    })
    expect(cli.posts).toContainEqual({ route: 'postal/claim', body: { workspace: '/ws/ada', address: 'ada::initech.postal.bot' } })
  })

  it('shows the address and no claim form once one is claimed', async () => {
    cli.status = statusBody({ id: 'h1', address: 'ada::initech.postal.bot', status: 'active', createdAt: '2026-11-17T00:00:00Z' })
    render(<PostalTab project={project} scope={scope} />)
    expect((await screen.findByTestId('postal-address')).textContent).toBe('ada::initech.postal.bot')
    expect(screen.queryByTestId('postal-claim-form')).toBeNull()
  })

  it('Accept posts postal/decide with the request id', async () => {
    cli.friends = [
      { id: 'f1', peer: 'scout::initech.postal.bot', status: 'pending_in', ownerName: 'Acme' },
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
    // Two groups with one even gap; the space stays in the text.
    const mine = await screen.findByTestId('postal-code-mine')
    expect(mine.textContent).toBe('123 456')
    expect(mine.className).toContain('gap-1')
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
    // Remove asks first, in words.
    const sheet = await screen.findByTestId('postal-remove-confirm')
    expect(sheet.textContent).toContain('Remove bee::hive.postal.bot?')
    expect(sheet.textContent).toContain('You’d need a new request to be friends again.')
    expect(cli.posts.find((p) => p.route === 'postal/remove')).toBeUndefined()
    await act(async () => {
      fireEvent.click(within(sheet).getByRole('button', { name: 'Remove' }))
    })
    expect(cli.posts).toContainEqual({ route: 'postal/remove', body: { workspace: '/ws/ada', peer: 'bee::hive.postal.bot' } })
    expect(screen.getByTestId('postal-notice').textContent).toBe('Removed bee::hive.postal.bot.')
  })

  it('Block asks first, says what it does, and Cancel blocks nothing', async () => {
    cli.friends = [{ id: 'f2', peer: 'bee::hive.postal.bot', status: 'accepted' }]
    render(<PostalTab project={project} scope={scope} />)
    const block = await screen.findByRole('button', { name: 'Block' })
    await act(async () => {
      fireEvent.click(block)
    })
    let sheet = await screen.findByTestId('postal-block-confirm')
    expect(sheet.textContent).toContain('Block bee::hive.postal.bot?')
    expect(sheet.textContent).toContain(
      'It ends the friendship and refuses its requests. You can unblock it later, but you’d need to be friends again.',
    )
    await act(async () => {
      fireEvent.click(within(sheet).getByRole('button', { name: 'Cancel' }))
    })
    expect(screen.queryByTestId('postal-block-confirm')).toBeNull()
    expect(cli.posts.find((p) => p.route === 'postal/block')).toBeUndefined()
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Block' }))
    })
    sheet = await screen.findByTestId('postal-block-confirm')
    await act(async () => {
      fireEvent.click(within(sheet).getByRole('button', { name: 'Block' }))
    })
    expect(cli.posts).toContainEqual({ route: 'postal/block', body: { workspace: '/ws/ada', peer: 'bee::hive.postal.bot' } })
    // Said in the Friends card, where it happened.
    const notice = screen.getByTestId('postal-notice')
    expect(notice.textContent).toContain('Blocked bee::hive.postal.bot.')
    expect(notice.closest('[data-settings-id="projects.postal-friends"]')).toBeTruthy()
  })

  it('an invitation the network has not sent yet says Not sent yet; a sent one says Waiting', async () => {
    cli.friends = [
      { id: 'o1', peer: 'vega::outpost.postal.bot', status: 'pending_out', pairId: null },
      { id: 'o2', peer: 'orbit::meridian.postal.bot', status: 'pending_out', pairId: 'pr_o2' },
    ]
    render(<PostalTab project={project} scope={scope} />)
    const rows = await screen.findAllByTestId('postal-peer-row')
    const byPeer = (peer: string): string => rows.find((r) => r.getAttribute('data-peer') === peer)?.textContent ?? ''
    expect(byPeer('vega::outpost.postal.bot')).toContain('Not sent yet')
    expect(byPeer('vega::outpost.postal.bot')).toContain('It goes out once this server connects to the Postal network.')
    expect(byPeer('orbit::meridian.postal.bot')).toContain('Waiting')
    expect(byPeer('orbit::meridian.postal.bot')).not.toContain('Not sent yet')
  })

  it("an incoming request shows its note in quotes, and a code request says you'll compare codes", async () => {
    cli.friends = [
      { id: 'f9', peer: 'lab::gamma.postal.bot', status: 'pending_in', ownerName: 'Dana', codeWaiting: true, note: 'Hi from Dana.' },
      { id: 'f8', peer: 'nova::quarry.postal.bot', status: 'pending_in' },
    ]
    render(<PostalTab project={project} scope={scope} />)
    const rows = await screen.findAllByTestId('postal-peer-row')
    const lab = rows.find((r) => r.getAttribute('data-peer') === 'lab::gamma.postal.bot') as HTMLElement
    const nova = rows.find((r) => r.getAttribute('data-peer') === 'nova::quarry.postal.bot') as HTMLElement
    expect(within(lab).getByTestId('postal-peer-note').textContent).toBe('Their note: “Hi from Dana.”')
    expect(within(lab).getByTestId('postal-row-explainer').textContent).toBe(
      'Code check: you’ll compare 6-digit codes with Dana before it’s accepted.',
    )
    expect(within(nova).queryByTestId('postal-row-explainer')).toBeNull()
    expect(within(nova).queryByTestId('postal-peer-note')).toBeNull()
  })

  it('a changed key says what it means and offers Trust new key', async () => {
    cli.friends = [{ id: 'f7', peer: 'iris::harbor.postal.bot', status: 'rekey_pending', ownerName: 'Ivo' }]
    render(<PostalTab project={project} scope={scope} />)
    const trust = await screen.findByRole('button', { name: 'Trust new key' })
    const row = trust.closest('[data-testid="postal-peer-row"]') as HTMLElement
    expect(within(row).getByTestId('postal-row-explainer').textContent).toContain('Trust the new key once Ivo says they changed it')
    await act(async () => {
      fireEvent.click(trust)
    })
    expect(cli.posts).toContainEqual({ route: 'postal/decide', body: { workspace: '/ws/ada', id: 'f7', decision: 'accept' } })
    expect(screen.getByTestId('postal-notice').textContent).toBe('Trusted iris::harbor.postal.bot’s new key.')
  })

  it('a request that stopped waiting reloads the lists and says so in plain words', async () => {
    cli.friends = [{ id: 'f8', peer: 'nova::quarry.postal.bot', status: 'pending_in' }]
    cli.postFail['postal/decide'] = new DaemonHttpError("this request isn't waiting for an answer (it is revoked)", 409, 'postal_not_pending')
    render(<PostalTab project={project} scope={scope} />)
    const accept = await screen.findByRole('button', { name: 'Accept' })
    const reads = cli.gets.filter((g) => g.route === 'postal/friends').length
    cli.friends = [{ id: 'f8', peer: 'nova::quarry.postal.bot', status: 'revoked' }]
    await act(async () => {
      fireEvent.click(accept)
    })
    await waitFor(() => expect(cli.gets.filter((g) => g.route === 'postal/friends').length).toBeGreaterThan(reads))
    const notice = screen.getByTestId('postal-notice')
    expect(notice.textContent).toBe(
      'The request from nova::quarry.postal.bot isn’t waiting for an answer any more: it was withdrawn or already answered. The list is up to date now.',
    )
    expect(notice.closest('[data-settings-id="projects.postal-invitations"]')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Accept' })).toBeNull()
    expect(document.body.textContent).not.toContain('(it is revoked)')
  })

  it('before the agent has an address, Invite shows only the one-line reason', async () => {
    render(<PostalTab project={project} scope={scope} />)
    const why = await screen.findByTestId('postal-invite-needs-address')
    expect(why.textContent).toContain('Claim a Postal address for this agent first.')
    expect(screen.queryByTestId('postal-invite-form')).toBeNull()
    expect(screen.queryByLabelText('Address to invite')).toBeNull()
  })

  it('Invitations has its own loading and error words', async () => {
    cli.getFail = new DaemonHttpError('the Postal store is busy', 503, 'postal_unreachable')
    render(<PostalTab project={project} scope={scope} />)
    const err = await screen.findByTestId('postal-invitations-error')
    expect(err.textContent).toContain('Couldn’t read this agent’s invitations')
    expect(screen.getByTestId('postal-friends-error').textContent).toContain('Couldn’t read this agent’s friends')
  })

  async function invite(to: string): Promise<void> {
    cli.status = statusBody({ id: 'h1', address: 'ada::initech.postal.bot', status: 'claimed', createdAt: '2026-11-17T00:00:00Z' })
    render(<PostalTab project={project} scope={scope} />)
    const field = await screen.findByLabelText('Address to invite')
    fireEvent.change(field, { target: { value: to } })
    await act(async () => {
      fireEvent.submit(screen.getByTestId('postal-invite-form'))
    })
  }

  it('the invite notice says sent when the daemon sent it', async () => {
    cli.inviteResult = { ok: true, sent: true, network: 'sent' }
    await invite('bee::hive.postal.bot')
    expect(cli.posts).toContainEqual({ route: 'postal/invite', body: { workspace: '/ws/ada', to: 'bee::hive.postal.bot' } })
    const notice = screen.getByTestId('postal-notice')
    expect(notice.textContent).toBe('Invitation sent to bee::hive.postal.bot.')
    expect(notice.closest('[data-settings-id="projects.postal-invitations"]')).toBeTruthy()
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

describe('PostalTab dates, route words and the code form', () => {
  it("reads the daemon's unix-seconds dates as real dates (not 1970)", async () => {
    // The daemon sends created/updated/decided times as unix seconds.
    cli.status = statusBody({ id: 'h1', address: 'ada::initech.postal.bot', status: 'claimed', createdAt: 1763500000 })
    cli.friends = [
      { id: 'f2', peer: 'bee::hive.postal.bot', status: 'accepted', createdAt: 1763500000, updatedAt: 1763500000, decidedAt: 1763500000 },
    ]
    render(<PostalTab project={project} scope={scope} />)
    await screen.findByTestId('postal-address')
    const tab = screen.getByTestId('postal-tab').textContent ?? ''
    expect(tab).toMatch(/claimed \D*\d{1,2}\D*2025/)
    expect(tab).not.toContain('1970')
    const row = (await screen.findAllByTestId('postal-peer-row'))[0]
    expect(row.textContent).toContain('2025')
    expect(row.textContent).not.toContain('1970')
  })

  it('also reads milliseconds and ISO strings, and shows nothing for junk', async () => {
    cli.friends = [
      { id: 'a', peer: 'ms::a.postal.bot', status: 'accepted', createdAt: 1763500000000 },
      { id: 'b', peer: 'iso::b.postal.bot', status: 'accepted', createdAt: '2025-11-18T21:00:00Z' },
      { id: 'c', peer: 'junk::c.postal.bot', status: 'accepted', createdAt: 'not a date' },
    ]
    render(<PostalTab project={project} scope={scope} />)
    const rows = await screen.findAllByTestId('postal-peer-row')
    expect(rows[0].textContent).toContain('2025')
    expect(rows[1].textContent).toContain('2025')
    expect(rows[2].textContent).not.toMatch(/Invalid|NaN|1970/)
  })

  it("says a friend's route in plain words, never the raw rule", async () => {
    cli.friends = [{ id: 'f2', peer: 'bee::hive.postal.bot', status: 'accepted', route: '{"targets":["terminal"],"wake":true}' }]
    render(<PostalTab project={project} scope={scope} />)
    const row = (await screen.findAllByTestId('postal-peer-row'))[0]
    expect(row.textContent).toContain('Mail: its terminal, wakes it')
    expect(row.textContent).not.toContain('{')
    expect(row.textContent).not.toContain('Route:')
  })

  it("the code form opens below the row, with the row's actions hidden; Back brings Reject back", async () => {
    cli.friends = [{ id: 'f9', peer: 'lab::gamma.postal.bot', status: 'pending_in', ownerName: 'Dana', codeWaiting: true }]
    cli.code = '123 456'
    render(<PostalTab project={project} scope={scope} />)
    const accept = await screen.findByRole('button', { name: 'Accept' })
    const row = accept.closest('[data-testid="postal-peer-row"]') as HTMLElement
    expect(row.querySelector('[data-testid="postal-row-actions"]')).toBeTruthy()
    await act(async () => {
      fireEvent.click(accept)
    })
    const form = await screen.findByTestId('postal-code-form')
    expect(row.contains(form)).toBe(true)
    expect(form.closest('[data-testid="postal-row-actions"]')).toBeNull()
    expect(form.closest('[data-testid="postal-row-below"]')).toBeTruthy()
    expect(row.querySelector('[data-testid="postal-row-actions"]')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Reject' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    expect(screen.queryByTestId('postal-code-form')).toBeNull()
    expect(screen.getByRole('button', { name: 'Reject' })).toBeTruthy()
  })

  it('says the peer is a friend after an accept with a code', async () => {
    cli.friends = [{ id: 'f9', peer: 'lab::gamma.postal.bot', status: 'pending_in', codeWaiting: true }]
    cli.code = '123 456'
    render(<PostalTab project={project} scope={scope} />)
    const accept = await screen.findByRole('button', { name: 'Accept' })
    await act(async () => {
      fireEvent.click(accept)
    })
    await screen.findByTestId('postal-code-mine')
    fireEvent.change(screen.getByTestId('postal-code-input'), { target: { value: '654321' } })
    await act(async () => {
      fireEvent.submit(screen.getByTestId('postal-code-form'))
    })
    expect(screen.getByTestId('postal-notice').textContent).toBe('lab::gamma.postal.bot is now a friend.')
  })
})

describe('postalRouteText', () => {
  it('says where mail lands and whether it wakes the agent', () => {
    expect(postalRouteText('{"targets":["terminal"],"wake":true}')).toBe('Mail: its terminal, wakes it')
    // The daemon reads a missing wake as on.
    expect(postalRouteText('{"targets":["terminal"]}')).toBe('Mail: its terminal, wakes it')
    expect(postalRouteText('{"targets":["inbox"],"wake":true}')).toBe('Mail: its inbox')
    expect(postalRouteText('{"targets":["inbox+pointer"],"wake":true}')).toBe('Mail: its inbox, with a note in its terminal')
    expect(postalRouteText('{"targets":["thread:rosson"],"wake":true}')).toBe('Mail: rosson’s Thread')
    expect(postalRouteText('{"targets":["thread:rosson","terminal"],"wake":false}')).toBe(
      'Mail: rosson’s Thread and its terminal, doesn’t wake it',
    )
    expect(postalRouteText('{"targets":["inbox"],"wake":false}')).toBe('Mail: its inbox, doesn’t wake it')
  })

  it("is null for the default (no rule) and for a rule it can't read", () => {
    expect(postalRouteText(null)).toBeNull()
    expect(postalRouteText(undefined)).toBeNull()
    expect(postalRouteText('')).toBeNull()
    expect(postalRouteText('junk')).toBeNull()
    expect(postalRouteText('{"targets":[]}')).toBeNull()
    expect(postalRouteText('{"targets":["pigeon"]}')).toBeNull()
    expect(postalRouteText('{"targets":"terminal"}')).toBeNull()
    expect(postalRouteText('{"targets":["thread:"]}')).toBeNull()
    expect(postalRouteText('[1,2]')).toBeNull()
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
      'This server connects to the Postal network to send and receive friend requests and your agents’ messages. Messages wait on the network until this server collects them, and anything sent while it isn’t connected waits until it is.',
    )
    expect(text).not.toMatch(/nothing\s+leaves/)
    expect(text).not.toMatch(/later release/)
  })

  it("the admins switch's hint fits the viewer and the switch", async () => {
    cli.settings = { enabled: true, adminsMayAccept: false }
    render(<PostalServerSwitch scope={scope} />)
    const hint = await screen.findByTestId('postal-admins-hint')
    await waitFor(() => expect(hint.textContent).toBe('Off: only you accept new friends for this server’s agents.'))
    cleanup()
    cli.role = 'admin'
    render(<PostalServerSwitch scope={scope} />)
    await screen.findByText(/Only this server.s owner can turn this on or off/)
    expect(screen.getByTestId('postal-admins-hint').textContent).toBe('Off: only the owner accepts new friends for this server’s agents.')
    cleanup()
    cli.settings = { enabled: true, adminsMayAccept: true }
    render(<PostalServerSwitch scope={scope} />)
    expect((await screen.findByTestId('postal-admins-hint')).textContent).toBe('On: admins accept new friends for this server’s agents too.')
  })

  it('a save that fails says so in plain words and offers Try again', async () => {
    cli.settings = { enabled: true, adminsMayAccept: false }
    cli.postFail['postal/settings'] = new DaemonHttpError('app settings: Read-only file system (os error 30)', 500, 'internal')
    render(<PostalServerSwitch scope={scope} />)
    const toggle = await screen.findByTestId('postal-admins-accept')
    await waitFor(() => expect((toggle as HTMLButtonElement).disabled).toBe(false))
    await act(async () => {
      fireEvent.click(toggle)
    })
    const failed = await screen.findByTestId('postal-save-failed')
    expect(failed.textContent).toContain('Couldn’t save that, so nothing changed. The server couldn’t do that just now.')
    expect(failed.textContent).not.toContain('os error')
    delete cli.postFail['postal/settings']
    await act(async () => {
      fireEvent.click(within(failed).getByRole('button', { name: 'Try again' }))
    })
    expect(cli.posts.filter((p) => p.route === 'postal/settings')).toHaveLength(2)
    expect(cli.settings.adminsMayAccept).toBe(true)
    expect(screen.queryByTestId('postal-save-failed')).toBeNull()
  })

  it('a non-owner sees it read-only', async () => {
    cli.role = 'admin'
    render(<PostalServerSwitch scope={scope} />)
    const toggle = await screen.findByTestId('postal-switch')
    await screen.findByText(/Only this server.s owner can turn this on or off/)
    expect((toggle as HTMLButtonElement).disabled).toBe(true)
  })
})

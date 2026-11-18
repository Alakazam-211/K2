// @vitest-environment jsdom
//
// "Get this server ready for Apps": the owner's card. Daemon calls are
// mocked; routes and bodies are asserted exactly. Fail loud: an unexpected
// route throws.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within, act } from '@testing-library/react'

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

import { AppsReadyOffer } from './AppsReadyOffer'
import { APPS_PAGE_EVENT, OPEN_EVENT, guestsText, localZone } from './apps-ready-api'

const OFFER = {
  trigger: 'upgrade',
  title: 'Get this server ready for Apps',
  line: 'Installs Podman and system packages, sets up the App user and firewall. No restart now: if one is needed, you pick when (a short restart signs guests out of older App sites).',
}
const BASE = { owner: true, linux: true, apps: true, ready: false, run: null, restartNeeded: false, restart: null, blocking: [] }

let served: Record<string, unknown> = { ...BASE, offer: OFFER }
const posts: Array<[string, unknown]> = []

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  posts.length = 0
  served = { ...BASE, offer: OFFER }
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'app/box/ready-offer') return served
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string, body: unknown) => {
    posts.push([route, body])
    if (route === 'app/box/ready-offer/dismiss') {
      served = { ...served, offer: null }
      return served
    }
    if (route === 'app/box/ready-offer/run') {
      served = { ...served, offer: null, run: { state: 'running', phase: 'prepare', blocking: [], changed: [] } }
      return served
    }
    if (route === 'app/box/ready-offer/trigger') return served
    if (route === 'app/box/ready-offer/restart') {
      const when = (body as { when: string }).when
      if (when === 'tonight') served = { ...served, restart: { at: 1_900_000_000, zone: 'America/Denver' } }
      if (when === 'cancel') served = { ...served, restart: null }
      return when === 'now' ? { ok: true, restarting: true } : served
    }
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => cleanup())

describe('the Apps setup card', () => {
  it('shows an owner the offer with what it does; Not now answers it and it does not come back', async () => {
    render(<AppsReadyOffer />)
    const sheet = await screen.findByTestId('apps-ready-offer')
    expect(within(sheet).getByTestId('apps-ready-line').textContent).toBe(OFFER.line)
    fireEvent.click(within(sheet).getByRole('button', { name: 'Not now' }))
    await waitFor(() => expect(posts).toEqual([['app/box/ready-offer/dismiss', { trigger: 'upgrade' }]]))
    expect(screen.queryByTestId('apps-ready-offer')).toBeNull()
    await act(async () => {
      window.dispatchEvent(new Event('focus'))
    })
    expect(screen.queryByTestId('apps-ready-offer')).toBeNull()
  })

  it('shows nothing to someone who is not an owner, and never runs anything by itself', async () => {
    served = { owner: false, offer: null }
    render(<AppsReadyOffer />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('app/box/ready-offer'))
    expect(screen.queryByTestId('apps-ready-offer')).toBeNull()
    expect(posts).toEqual([])
  })

  it('runs only on the click, shows progress, then offers a deferred restart with who would be signed out', async () => {
    render(<AppsReadyOffer />)
    const sheet = await screen.findByTestId('apps-ready-offer')
    expect(posts).toEqual([])
    fireEvent.click(within(sheet).getByRole('button', { name: 'Get ready' }))
    await waitFor(() => expect(posts).toEqual([['app/box/ready-offer/run', {}]]))
    expect((await screen.findByTestId('apps-ready-progress')).textContent).toContain('Installing Podman and system packages')
    served = {
      ...served,
      ready: true,
      restartNeeded: true,
      guests: { connected: 0, signedIn: 4 },
      run: { state: 'done', phase: 'finished', ready: true, restartNeeded: true, blocking: [], changed: ['apps ensure-user'] },
    }
    await act(async () => {
      window.dispatchEvent(new Event('focus'))
    })
    expect((await screen.findByTestId('apps-ready-done')).textContent).toBe('This server is ready for Apps.')
    expect(screen.getByTestId('apps-ready-guests').textContent).toContain('Nobody is connected to your Apps right now')
    expect(posts.filter(([r]) => r === 'app/box/ready-offer/restart')).toEqual([])
    fireEvent.click(screen.getByRole('button', { name: 'Tonight at 3 am' }))
    const tz = localZone()
    await waitFor(() => expect(posts.at(-1)).toEqual(['app/box/ready-offer/restart', tz ? { when: 'tonight', tz } : { when: 'tonight' }]))
    expect(await screen.findByTestId('apps-ready-scheduled')).toBeTruthy()
    expect(posts.some(([r, b]) => r === 'app/box/ready-offer/restart' && (b as { when: string }).when === 'now')).toBe(false)
    fireEvent.click(screen.getByRole('button', { name: 'Restart now' }))
    await waitFor(() => expect(posts.at(-1)).toEqual(['app/box/ready-offer/restart', { when: 'now' }]))
  })

  it('names the checks that still block after a run', async () => {
    served = {
      ...BASE,
      offer: null,
      run: { state: 'done', phase: 'finished', ready: false, restartNeeded: false, changed: [], blocking: [{ id: 'nft', status: 'fail', detail: 'table k2app missing', fix: 'k2 app box prepare (owner)' }] },
    }
    render(<AppsReadyOffer />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalled())
    expect(screen.queryByTestId('apps-ready-offer')).toBeNull()
    await act(async () => {
      window.dispatchEvent(new Event(OPEN_EVENT))
    })
    const list = await screen.findByTestId('apps-ready-blocking')
    expect(list.textContent).toContain('table k2app missing (fix: k2 app box prepare (owner))')
    expect(screen.getByRole('button', { name: 'Run it again' })).toBeTruthy()
  })

  it('tells the daemon when the Apps page opens', async () => {
    served = { ...BASE, offer: null }
    render(<AppsReadyOffer />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalled())
    served = { ...BASE, offer: { ...OFFER, trigger: 'apps_page' } }
    await act(async () => {
      window.dispatchEvent(new Event(APPS_PAGE_EVENT))
    })
    await waitFor(() => expect(posts).toEqual([['app/box/ready-offer/trigger', { trigger: 'apps_page' }]]))
    expect(await screen.findByTestId('apps-ready-offer')).toBeTruthy()
  })
})

describe('guestsText', () => {
  it('says whether a restart signs anyone out', () => {
    expect(guestsText({ connected: 0, signedIn: 3 })).toBe('Nobody is connected to your Apps right now, so a restart signs nobody out.')
    expect(guestsText({ connected: 1, signedIn: 3 })).toBe('1 guest is connected to your Apps right now and would be signed out of older App sites.')
    expect(guestsText({ connected: 4, signedIn: 9 })).toBe('4 guests are connected to your Apps right now and would be signed out of older App sites.')
  })
})

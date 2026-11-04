// @vitest-environment jsdom
// People's pictures are per server (Rosson 2026-11-04): a person chip shows
// the picture the server that owns where it sits keeps for that login, read
// with that server's own scope; the same login on two servers can show two
// pictures. A chip in a room reads its room's server; one given
// `pictureScope` reads that; one outside both reads the window's server.
// Subscriptions are counted and dropped when the last chip unmounts.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, waitFor } from '@testing-library/react'

const d = vi.hoisted(() => ({
  /** hostKey → login → picture tag. */
  servers: {} as Record<string, Record<string, string>>,
  gets: [] as Array<{ host: string; params?: Record<string, unknown> }>,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: Record<string, unknown>) => {
    if (route !== 'people/picture') throw new Error(`unexpected GET ${route}`)
    d.gets.push({ host: scope.hostKey, params })
    const pics = d.servers[scope.hostKey]
    if (!pics) throw new Error('route not found')
    const who = params?.person
    if (typeof who === 'string') {
      const tag = pics[who]
      return { ok: true, person: who, imageHash: tag ? `h-${tag}` : null, imageUrl: tag ? `data:image/jpeg;base64,${btoa(tag)}` : null }
    }
    return { ok: true, people: Object.entries(pics).map(([login, tag]) => ({ person: login, logins: [login], imageHash: `h-${tag}` })) }
  }),
  daemonCliPost: vi.fn(async () => {
    throw new Error('no writes here')
  }),
}))

const primary = vi.hoisted(() => ({ id: 'primary', hostKey: 'local', isPrimary: true, label: 'This computer' }))
vi.mock('@/kessel/server-scope', async (orig) => ({
  ...(await orig<typeof import('@/kessel/server-scope')>()),
  primaryScope: () => primary,
}))

import { PresenceChip } from '@/components/Presence/PresenceChip'
import { PictureScopeContext } from '@/contexts/PictureScopeContext'
import { resetPersonPicturesForTests, parsePictureList } from './person-pictures'
import { appBusHandlerCount } from '@/stores/session-events'
import type { ServerScope } from '@/kessel/server-scope'

const scopeA = { id: 'host:a.k2.dev', hostKey: 'a.k2.dev', isPrimary: false, label: 'A' } as unknown as ServerScope
const scopeB = { id: 'host:b.k2.dev', hostKey: 'b.k2.dev', isPrimary: false, label: 'B' } as unknown as ServerScope

function painted(testId: string): string | null {
  const chip = document.querySelector(`[data-testid="${testId}"] [data-presence-chip]`)
  if (!chip) throw new Error(`no chip ${testId}`)
  const span = (Array.from(chip.querySelectorAll('span')) as HTMLElement[]).find((s) => s.style.backgroundImage)
  return span ? span.style.backgroundImage : null
}

beforeEach(() => {
  resetPersonPicturesForTests()
  d.gets = []
  d.servers = {
    local: { anna: 'anna-local', owner: 'owner-local' },
    'a.k2.dev': { anna: 'anna-on-a' },
    'b.k2.dev': { anna: 'anna-on-b' },
  }
})

afterEach(() => cleanup())

describe('person pictures are per server', () => {
  it('the same login shows each server\'s own picture, read from that server', async () => {
    render(
      <>
        <span data-testid="a">
          <PresenceChip name="anna" role="member" pictureScope={scopeA} />
        </span>
        <span data-testid="b">
          <PresenceChip name="anna" role="member" pictureScope={scopeB} />
        </span>
        <span data-testid="window">
          <PresenceChip name="anna" role="member" />
        </span>
      </>,
    )
    await waitFor(() => expect(painted('a')).toContain(`data:image/jpeg;base64,${btoa('anna-on-a')}`))
    await waitFor(() => expect(painted('b')).toContain(`data:image/jpeg;base64,${btoa('anna-on-b')}`))
    await waitFor(() => expect(painted('window')).toContain(`data:image/jpeg;base64,${btoa('anna-local')}`))
    // Every read went to the server the chip belongs to.
    for (const host of ['a.k2.dev', 'b.k2.dev', 'local']) {
      expect(d.gets.filter((g) => g.host === host).map((g) => g.params?.person ?? null)).toEqual([null, 'anna'])
    }
  })

  it('a chip in a room reads its room\'s server; pictureScope null shows none', async () => {
    render(
      <PictureScopeContext.Provider value={scopeB}>
        <span data-testid="in-room">
          <PresenceChip name="anna" role="member" />
        </span>
        <span data-testid="none">
          <PresenceChip name="anna" role="member" pictureScope={null} />
        </span>
      </PictureScopeContext.Provider>,
    )
    await waitFor(() => expect(painted('in-room')).toContain(btoa('anna-on-b')))
    expect(painted('none')).toBeNull()
    expect(d.gets.every((g) => g.host === 'b.k2.dev')).toBe(true)
  })

  it('a server without the route shows initials', async () => {
    delete d.servers['a.k2.dev']
    render(
      <span data-testid="old">
        <PresenceChip name="anna" role="member" pictureScope={scopeA} />
      </span>,
    )
    await waitFor(() => expect(d.gets.length).toBe(1))
    expect(painted('old')).toBeNull()
    expect(document.querySelector('[data-testid="old"] [data-presence-chip]')?.textContent).toBe('A')
  })

  it('the last chip on a server unmounting drops its subscriptions', async () => {
    const before = appBusHandlerCount(scopeA)
    const view = render(
      <>
        <PresenceChip name="anna" role="member" pictureScope={scopeA} />
        <PresenceChip name="owner" role="owner" pictureScope={scopeA} />
      </>,
    )
    await waitFor(() => expect(appBusHandlerCount(scopeA)).toBe(before + 2))
    view.unmount()
    expect(appBusHandlerCount(scopeA)).toBe(before)
  })

  it('an owner-linked login shows the owner\'s picture', () => {
    expect(parsePictureList({ people: [{ person: 'owner', logins: ['owner', 'Rosson'], imageHash: 'h1' }, { person: 'x', imageHash: '' }] })).toEqual({
      owner: 'h1',
      rosson: 'h1',
    })
  })
})

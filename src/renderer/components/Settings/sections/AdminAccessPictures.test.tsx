// @vitest-environment jsdom
// Admin Access: people's pictures (0.46.8, Rosson 2026-11-04). Each row
// shows the person's picture on this server (initials until they have
// one); "Add picture" / "Change picture" / "Remove picture" set or remove
// it through POST /cli/people/picture[/clear] with `person` (the daemon
// applies Admin Access's rule and says why on a refusal); a change made
// anywhere arrives as `person_picture_changed` and the row repaints.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

type Json = Record<string, unknown>

const PNG = 'data:image/png;base64,iVBORw0KGgo='
const jpeg = (tag: string): string => `data:image/jpeg;base64,${btoa(tag)}`

const cli = vi.hoisted(() => ({
  users: [] as Json[],
  /** login → {hash, url}: the daemon's pictures. */
  pictures: {} as Record<string, { hash: string; url: string }>,
  gets: [] as Array<{ route: string; params?: Json }>,
  posts: [] as Array<{ route: string; body: Json }>,
  refuse: null as string | null,
  pictureHandlers: new Set<(e: unknown) => void>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main', listen: async () => () => undefined }),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Json) => {
    cli.gets.push({ route, params })
    if (route === 'owner/accounts') return { ok: true, accounts: [], aliases: [] }
    if (route === 'people/picture') {
      const who = params?.person
      if (typeof who === 'string') {
        const p = cli.pictures[who]
        return { ok: true, person: who, imageHash: p?.hash ?? null, imageUrl: p?.url ?? null }
      }
      return {
        ok: true,
        people: Object.entries(cli.pictures).map(([login, p]) => ({ person: login, logins: [login], imageHash: p.hash })),
      }
    }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Json) => {
    cli.posts.push({ route, body })
    if (cli.refuse) throw new Error(cli.refuse)
    const who = String(body.person)
    if (route === 'people/picture') {
      const p = { hash: `h-${who}-${cli.posts.length}`, url: jpeg(`${who}-${cli.posts.length}`) }
      cli.pictures[who] = p
      return { ok: true, person: who, imageHash: p.hash, imageUrl: p.url }
    }
    if (route === 'people/picture/clear') {
      delete cli.pictures[who]
      return { ok: true, person: who, imageHash: null, imageUrl: null }
    }
    throw new Error(`unexpected POST ${route}`)
  }),
}))

vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onPersonPictureChanged: (_scope: unknown, fn: (e: unknown) => void) => {
    cli.pictureHandlers.add(fn)
    return () => void cli.pictureHandlers.delete(fn)
  },
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 1, token: 'tok' })),
  getLocalDaemonWs: vi.fn(async () => ({ port: 1, token: 'tok' })),
  daemonHttpBase: () => 'http://daemon.test',
  invalidateDaemonWs: vi.fn(),
}))

vi.mock('./useNotifyPrefs', async (orig) => ({
  ...(await orig<typeof import('./useNotifyPrefs')>()),
  threadPushAvailable: () => true,
}))

import { K2ConnectSection } from './K2ConnectSection'
import { resetPersonPicturesForTests } from '@/lib/person-pictures'

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

function chip(login: string): HTMLElement {
  const el = screen.getByTestId(`user-chip-${login}`).querySelector('[data-presence-chip]')
  if (!(el instanceof HTMLElement)) throw new Error(`no chip for ${login}`)
  return el
}

function hasPicture(login: string): boolean {
  return chip(login).hasAttribute('data-person-picture')
}

function pictureOf(login: string): string {
  const painted = (Array.from(chip(login).querySelectorAll('span')) as HTMLElement[]).find((s) => s.style.backgroundImage)
  if (!painted) throw new Error(`no picture painted for ${login}`)
  return painted.style.backgroundImage
}

function pick(login: string, file: File): void {
  const input = screen.getByTestId(`user-picture-${login}-file`) as HTMLInputElement
  act(() => void fireEvent.change(input, { target: { files: [file] } }))
}

function fire(person: string, imageHash: string | null): void {
  act(() => {
    for (const fn of cli.pictureHandlers) fn({ kind: 'person_picture_changed', person, imageHash })
  })
}

beforeEach(() => {
  resetPersonPicturesForTests()
  cli.users = [
    { username: 'z3thon', disabled: false, role: 'admin', firstName: 'Rosson', lastName: 'Long', displayName: 'Rosson Long' },
    { username: 'gina', disabled: false, role: 'member', firstName: null, lastName: null, displayName: 'gina' },
  ]
  cli.pictures = { z3thon: { hash: 'h-z', url: jpeg('z3thon') } }
  cli.gets = []
  cli.posts = []
  cli.refuse = null
  cli.pictureHandlers.clear()
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET'
      const path = new URL(url).pathname
      if (path === '/cli/auth/whoami') return json({ owner: true, role: 'owner' })
      if (path === '/cli/users' && method === 'GET') return json({ users: cli.users })
      if (path === '/cli/users/policy') return json({ minLength: 8 })
      throw new Error(`unexpected fetch ${method} ${path}`)
    }),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('Admin Access: people\'s pictures', () => {
  it('each row shows that person\'s picture on this server, initials without one', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('user-chip-z3thon')
    await waitFor(() => expect(hasPicture('z3thon')).toBe(true))
    expect(pictureOf('z3thon')).toContain(jpeg('z3thon'))
    expect(hasPicture('gina')).toBe(false)
    expect(chip('gina').textContent).toBe('G')
    expect(screen.getByTestId('user-picture-z3thon-choose').textContent).toBe('Change picture')
    expect(screen.getByTestId('user-picture-gina-choose').textContent).toBe('Add picture')
    expect(document.querySelector('[data-testid="user-picture-gina-remove"]')).toBeNull()
  })

  it('Add picture sends the file for that person; the row shows the daemon\'s re-encode', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('user-chip-gina')
    pick('gina', new File([Uint8Array.from(atob('iVBORw0KGgo='), (c) => c.charCodeAt(0))], 'me.png', { type: 'image/png' }))
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'people/picture', body: { image: PNG, person: 'gina' } }]))
    await waitFor(() => expect(hasPicture('gina')).toBe(true))
    expect(pictureOf('gina')).toContain(cli.pictures.gina.url)
    expect(screen.getByTestId('user-picture-gina-choose').textContent).toBe('Change picture')
  })

  it('Remove picture clears it for that person', async () => {
    render(<K2ConnectSection panel="people" />)
    await waitFor(() => expect(hasPicture('z3thon')).toBe(true))
    act(() => void fireEvent.click(screen.getByTestId('user-picture-z3thon-remove')))
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'people/picture/clear', body: { person: 'z3thon' } }]))
    await waitFor(() => expect(hasPicture('z3thon')).toBe(false))
    expect(chip('z3thon').textContent).toBe('RL')
  })

  it('a refusal shows the daemon\'s reason and changes nothing', async () => {
    cli.refuse = "you can change only your own picture (an Admin can change a Member's or an Admin's; only the owner can change an Owner's)"
    render(<K2ConnectSection panel="people" />)
    await waitFor(() => expect(hasPicture('z3thon')).toBe(true))
    act(() => void fireEvent.click(screen.getByTestId('user-picture-z3thon-remove')))
    expect((await screen.findByTestId('user-picture-z3thon-error')).textContent).toBe(cli.refuse)
    expect(hasPicture('z3thon')).toBe(true)
  })

  it('a file of the wrong type or over 2 MB is refused before anything is sent', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('user-chip-gina')
    pick('gina', new File(['<svg/>'], 'me.svg', { type: 'image/svg+xml' }))
    expect(screen.getByTestId('user-picture-gina-error').textContent).toBe('Pick a PNG, JPEG or WebP picture.')
    pick('gina', new File([new Uint8Array(2 * 1024 * 1024 + 1)], 'big.png', { type: 'image/png' }))
    expect(screen.getByTestId('user-picture-gina-error').textContent).toBe('That picture is over 2 MB. Pick a smaller one.')
    expect(cli.posts).toEqual([])
  })

  it('a change made elsewhere arrives as person_picture_changed and the row repaints', async () => {
    render(<K2ConnectSection panel="people" />)
    await waitFor(() => expect(hasPicture('z3thon')).toBe(true))
    expect(cli.pictureHandlers.size).toBeGreaterThan(0)
    // gina sets hers on her phone; z3thon's is removed by the owner.
    cli.pictures = { gina: { hash: 'h-g2', url: jpeg('gina-phone') } }
    fire('gina', 'h-g2')
    await waitFor(() => expect(hasPicture('gina')).toBe(true))
    expect(pictureOf('gina')).toContain(jpeg('gina-phone'))
    await waitFor(() => expect(hasPicture('z3thon')).toBe(false))
  })
})

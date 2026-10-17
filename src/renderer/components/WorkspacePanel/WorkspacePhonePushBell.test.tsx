// @vitest-environment jsdom
// 0.46.0 (Rosson 2026-10-16): the Workspace drawer header's Thread push
// toggle, beside the completion bell. It reads and writes the caller's own
// prefs on the ROOM's server (POST /cli/notify/prefs), flips optimistically
// and rolls back on error, says where an inherited value came from, dims
// while the server bell is off, explains dormant / air-gap, hides on a server
// without `thread-push-v1`, and is read-only in a view-only room.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'

type Json = Record<string, unknown>

const cli = vi.hoisted(() => ({
  prefs: {} as Json,
  gets: [] as Array<{ scope: string; route: string }>,
  posts: [] as Array<{ scope: string; route: string; body: Json }>,
  /** When set, the next POST waits on this and then answers / throws. */
  gate: null as null | { promise: Promise<void> },
  failPost: false,
}))
const events = vi.hoisted(() => ({ handlers: new Set<() => void>() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { id: string }, route: string) => {
    cli.gets.push({ scope: scope.id, route })
    if (route === 'notify/prefs') return cli.prefs
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (scope: { id: string }, route: string, body: Json) => {
    cli.posts.push({ scope: scope.id, route, body })
    if (route !== 'notify/prefs') throw new Error(`unexpected POST ${route}`)
    if (cli.gate) await cli.gate.promise
    if (cli.failPost) throw new Error('daemon said no')
    return cli.prefs
  }),
}))

vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onNotifyPrefsChanged: (_scope: unknown, fn: () => void) => {
    events.handlers.add(fn)
    return () => void events.handlers.delete(fn)
  },
}))

import { WorkspacePhonePushBell } from './WorkspacePhonePushToggle'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import type { ServerScope } from '@/kessel/server-scope'

function prefsWith(row: Json, extra: Json = {}): Json {
  return {
    ok: true,
    principal: 'owner',
    server: { enabled: true, source: 'fallback' },
    default: true,
    devices: 1,
    push: 'ready',
    workspaces: [
      { projectId: 'p-other', name: 'other', handle: 'other', threadPush: false, source: 'user', updatedBy: 'owner' },
      { projectId: 'p-k2', name: 'k2', handle: 'k2', ...row },
    ],
    ...extra,
  }
}

function render(opts: { scope?: ServerScope; readOnly?: boolean } = {}): void {
  const room = testRoom({
    tabs: createStore(() => ({})),
    isPrimary: false,
    scope: opts.scope ?? fakeScope('b.k2.dev'),
    readOnly: opts.readOnly ?? false,
    key: 'b',
  })
  renderInRoom(room, <WorkspacePhonePushBell project={{ id: 'p-k2' }} />)
}

async function phone(): Promise<HTMLButtonElement> {
  return (await screen.findByTestId('workspace-phone-push-bell')) as HTMLButtonElement
}

beforeEach(() => {
  cli.prefs = prefsWith({ threadPush: true, source: 'user', updatedBy: 'owner' })
  cli.gets = []
  cli.posts = []
  cli.gate = null
  cli.failPost = false
  events.handlers.clear()
})

afterEach(() => {
  cleanup()
})

describe('Workspace drawer phone push toggle', () => {
  it('reads the room’s server and shows this workspace’s own value (accent, phone icon)', async () => {
    render()
    const btn = await phone()
    expect(cli.gets).toEqual([{ scope: 'host:b.k2.dev', route: 'notify/prefs' }])
    expect(btn.getAttribute('aria-checked')).toBe('true')
    expect(btn.className).toContain('text-[var(--color-accent)]')
    expect(btn.title).toBe('Thread pushes to your phone: on')
    expect(btn.querySelector('[data-k2-icon="smartphone"]')).not.toBeNull()
  })

  it('off shows the muted phone with a slash', async () => {
    cli.prefs = prefsWith({ threadPush: false, source: 'user', updatedBy: 'owner' })
    render()
    const btn = await phone()
    expect(btn.getAttribute('aria-checked')).toBe('false')
    expect(btn.className).toContain('text-[var(--color-text-muted)]')
    expect(btn.title).toBe('Thread pushes to your phone: off')
    expect(btn.querySelector('[data-k2-icon="smartphone-off"]')).not.toBeNull()
  })

  it('a click posts the caller’s own row for this workspace to the room’s server and flips', async () => {
    render()
    const btn = await phone()
    let release!: () => void
    cli.gate = { promise: new Promise<void>((r) => (release = r)) }
    cli.prefs = prefsWith({ threadPush: false, source: 'user', updatedBy: 'owner' })
    fireEvent.click(btn)
    // Optimistic: off before the daemon answers.
    expect(btn.getAttribute('aria-checked')).toBe('false')
    expect(cli.posts).toEqual([
      { scope: 'host:b.k2.dev', route: 'notify/prefs', body: { workspace: { projectId: 'p-k2', threadPush: false } } },
    ])
    await act(async () => release())
    await waitFor(() => expect(btn.disabled).toBe(false))
    expect(btn.getAttribute('aria-checked')).toBe('false')
    expect(btn.title).toBe('Thread pushes to your phone: off')
  })

  it('rolls back when the write fails', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    render()
    const btn = await phone()
    let release!: () => void
    cli.gate = { promise: new Promise<void>((r) => (release = r)) }
    cli.failPost = true
    fireEvent.click(btn)
    expect(btn.getAttribute('aria-checked')).toBe('false')
    await act(async () => release())
    await waitFor(() => expect(btn.disabled).toBe(false))
    expect(btn.getAttribute('aria-checked')).toBe('true')
    expect(btn.title).toBe('Thread pushes to your phone: on')
    expect(cli.posts).toHaveLength(1)
  })

  it('says when the value is inherited from the workspace default or the fallback', async () => {
    cli.prefs = prefsWith({ threadPush: false, source: 'default' })
    render()
    expect((await phone()).title).toBe('Thread pushes to your phone: off (workspace default)')
    cleanup()
    cli.prefs = prefsWith({ threadPush: true, source: 'fallback' })
    render()
    expect((await phone()).title).toBe('Thread pushes to your phone: on (default)')
  })

  it('an inherited value clicked becomes the caller’s own (no longer says default)', async () => {
    cli.prefs = prefsWith({ threadPush: true, source: 'default' })
    render()
    const btn = await phone()
    cli.prefs = prefsWith({ threadPush: false, source: 'user', updatedBy: 'owner' })
    fireEvent.click(btn)
    await waitFor(() => expect(btn.disabled).toBe(false))
    expect(btn.title).toBe('Thread pushes to your phone: off')
  })

  it('dims while the server bell is off and says so, but still flips the workspace value', async () => {
    cli.prefs = prefsWith({ threadPush: true, source: 'user' }, { server: { enabled: false, source: 'user' } })
    render()
    const btn = await phone()
    expect(btn.className).toContain('opacity-40')
    expect(btn.dataset.serverBell).toBe('off')
    expect(btn.title.split('\n')).toEqual([
      'Your phone pushes are off for this server (Settings → Notifications)',
      'Thread pushes to your phone: on',
    ])
    fireEvent.click(btn)
    expect(cli.posts.map((p) => p.body)).toEqual([{ workspace: { projectId: 'p-k2', threadPush: false } }])
  })

  it('is not dimmed while the server bell is on', async () => {
    render()
    const btn = await phone()
    expect(btn.className).not.toContain('opacity-40')
    expect(btn.dataset.serverBell).toBe('on')
  })

  it('dormant and air-gap add a line to the tooltip', async () => {
    cli.prefs = prefsWith({ threadPush: true, source: 'user' }, { push: 'dormant' })
    render()
    expect((await phone()).title.split('\n')).toEqual([
      'Thread pushes to your phone: on',
      "This server isn't connected to the push service yet",
    ])
    cleanup()
    cli.prefs = prefsWith({ threadPush: true, source: 'user' }, { push: 'airgap' })
    render()
    expect((await phone()).title.split('\n')).toEqual(['Thread pushes to your phone: on', 'Air-gapped: no pushes'])
  })

  it('is hidden, and fetches nothing, on a server without thread-push-v1', async () => {
    const old: ServerScope = { ...fakeScope('old.k2.dev'), serverSupports: () => false }
    render({ scope: old })
    await act(async () => {})
    expect(screen.queryByTestId('workspace-phone-push-bell')).toBeNull()
    expect(cli.gets).toEqual([])
  })

  it('is hidden when the prefs read fails (no notify routes)', async () => {
    const { daemonCliGet } = await import('@/lib/daemon-cli')
    vi.mocked(daemonCliGet).mockClear()
    vi.mocked(daemonCliGet).mockRejectedValueOnce(new Error('404 not found'))
    render()
    await waitFor(() => expect(vi.mocked(daemonCliGet)).toHaveBeenCalledTimes(1))
    await act(async () => {})
    expect(screen.queryByTestId('workspace-phone-push-bell')).toBeNull()
  })

  it('a view-only room shows it read-only: a click writes nothing', async () => {
    render({ readOnly: true })
    const btn = await phone()
    expect(btn.getAttribute('aria-readonly')).toBe('true')
    expect(btn.title.split('\n')).toContain('View only: you cannot change it from this room')
    fireEvent.click(btn)
    await act(async () => {})
    expect(cli.posts).toEqual([])
    expect(btn.getAttribute('aria-checked')).toBe('true')
  })

  it('refetches on notify_prefs_changed (Settings → Notifications wrote)', async () => {
    render()
    const btn = await phone()
    cli.prefs = prefsWith({ threadPush: false, source: 'user', updatedBy: 'owner' })
    await act(async () => {
      for (const fn of events.handlers) fn()
    })
    await waitFor(() => expect(btn.getAttribute('aria-checked')).toBe('false'))
    expect(cli.gets).toHaveLength(2)
  })
})

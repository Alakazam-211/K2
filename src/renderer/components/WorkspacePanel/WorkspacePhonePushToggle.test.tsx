// @vitest-environment jsdom
// Workspaces → ws → Agent tab: "Phone notifications (you)"
// (prd-thread-push-notifications-v1 §9 shortcut, TP23). It edits only the
// caller's own row (POST /cli/notify/prefs), shows the effective value and
// where it came from, offers Reset on the caller's own row, refetches on
// `notify_prefs_changed`, and hides on a server without `thread-push-v1`.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

type Json = Record<string, unknown>

const cli = vi.hoisted(() => ({
  prefs: {} as Json,
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: Json }>,
}))
const events = vi.hoisted(() => ({ handlers: new Set<() => void>() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    cli.gets.push(route)
    if (route === 'notify/prefs') return cli.prefs
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Json) => {
    cli.posts.push({ route, body })
    if (route !== 'notify/prefs') throw new Error(`unexpected POST ${route}`)
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

import { WorkspacePhonePushToggle } from './WorkspacePhonePushToggle'
import { noteServerVersion, __resetServerScopesForTests } from '@/kessel/server-scope'

function prefsWith(row: Json, server = true): Json {
  return {
    ok: true,
    principal: 'owner',
    server: { enabled: server, source: 'fallback' },
    default: true,
    devices: 1,
    push: 'ready',
    workspaces: [
      { projectId: 'p-other', name: 'other', handle: 'other', threadPush: false, source: 'user', updatedBy: 'owner' },
      { projectId: 'p-k2', name: 'k2', handle: 'k2', ...row },
    ],
  }
}

beforeEach(() => {
  __resetServerScopesForTests()
  noteServerVersion('local', null, ['thread-push-v1'])
  cli.prefs = prefsWith({ threadPush: true, source: 'fallback' })
  cli.gets = []
  cli.posts = []
  events.handlers.clear()
})

afterEach(() => {
  cleanup()
  __resetServerScopesForTests()
})

function sw(): HTMLButtonElement {
  return screen.getByRole('switch', { name: 'Phone notifications from this workspace' }) as HTMLButtonElement
}

describe('Agent tab: Phone notifications (you)', () => {
  it('is hidden, and fetches nothing, on a server without thread-push-v1', async () => {
    noteServerVersion('local', null, [])
    const { container } = render(<WorkspacePhonePushToggle project={{ id: 'p-k2' }} />)
    await act(async () => {
      await Promise.resolve()
    })
    expect(container.innerHTML).toBe('')
    expect(cli.gets).toEqual([])
  })

  it('shows this workspace’s effective value and source (not another workspace’s)', async () => {
    cli.prefs = prefsWith({ threadPush: false, source: 'default' })
    render(<WorkspacePhonePushToggle project={{ id: 'p-k2' }} />)
    await waitFor(() => expect(screen.getByTestId('workspace-phone-push-source').textContent).toBe('Default (off)'))
    expect(sw().getAttribute('aria-checked')).toBe('false')
    expect(screen.queryByText('Reset')).toBeNull()
  })

  it('flipping it writes only the caller’s own row for this workspace', async () => {
    render(<WorkspacePhonePushToggle project={{ id: 'p-k2' }} />)
    await waitFor(() => expect(sw().disabled).toBe(false))
    cli.prefs = prefsWith({ threadPush: false, source: 'user', updatedBy: 'owner' })
    fireEvent.click(sw())
    await waitFor(() => expect(screen.getByTestId('workspace-phone-push-source').textContent).toBe('Set by you'))
    expect(sw().getAttribute('aria-checked')).toBe('false')
    expect(cli.posts).toEqual([{ route: 'notify/prefs', body: { workspace: { projectId: 'p-k2', threadPush: false } } }])
  })

  it('"Set by gina" with Reset; Reset sends threadPush:null', async () => {
    cli.prefs = prefsWith({ threadPush: false, source: 'user', updatedBy: 'gina' })
    render(<WorkspacePhonePushToggle project={{ id: 'p-k2' }} />)
    await waitFor(() => expect(screen.getByTestId('workspace-phone-push-source').textContent).toBe('Set by gina'))
    cli.prefs = prefsWith({ threadPush: true, source: 'fallback' })
    fireEvent.click(screen.getByText('Reset'))
    await waitFor(() => expect(screen.getByTestId('workspace-phone-push-source').textContent).toBe('Default (on)'))
    expect(cli.posts).toEqual([{ route: 'notify/prefs', body: { workspace: { projectId: 'p-k2', threadPush: null } } }])
    expect(screen.queryByText('Reset')).toBeNull()
  })

  it('says when the server bell is off', async () => {
    cli.prefs = prefsWith({ threadPush: true, source: 'fallback' }, false)
    render(<WorkspacePhonePushToggle project={{ id: 'p-k2' }} />)
    await waitFor(() => expect(screen.getByText('Settings → Notifications')).toBeTruthy())
  })

  it('refetches on notify_prefs_changed', async () => {
    render(<WorkspacePhonePushToggle project={{ id: 'p-k2' }} />)
    await waitFor(() => expect(sw().getAttribute('aria-checked')).toBe('true'))
    cli.prefs = prefsWith({ threadPush: false, source: 'user', updatedBy: 'owner' })
    act(() => {
      for (const fn of events.handlers) fn()
    })
    await waitFor(() => expect(sw().getAttribute('aria-checked')).toBe('false'))
    expect(cli.gets.filter((g) => g === 'notify/prefs')).toHaveLength(2)
  })
})

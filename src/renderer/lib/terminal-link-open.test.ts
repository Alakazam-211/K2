// @vitest-environment jsdom
// A link clicked in a terminal: web links go to the opener (as before),
// a file:// link inside one of the room's workspaces opens in K2's file
// viewer, and anything else is refused with a toast, never silently.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ posts: [] as Array<{ route: string; body: unknown }> }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
    h.posts.push({ route, body })
    return {}
  }),
}))
vi.mock('@/kessel/server-scope', () => ({ scopeMayWrite: () => true }))

import {
  fileUrlPath,
  insideAnyRoot,
  openTerminalUrl,
  planTerminalUrl,
  terminalWorkspaceRoots,
  TERMINAL_LINK_REFUSED,
} from './terminal-link-open'
import { useToastStore } from '@/stores/toast'

const ROOTS = ['/Users/me/ws', '/Users/me/ws-wt/feature', 'C:\\Users\\me\\proj']

function room() {
  const projects = [
    { path: '/Users/me/ws', workspaces: [{ worktreePath: '/Users/me/ws-wt/feature' }, { worktreePath: null }] },
    { path: 'C:\\Users\\me\\proj', workspaces: [] },
  ]
  const store = { getState: () => ({ projects }), getInitialState: () => ({ projects }), subscribe: () => () => {} }
  return { isPrimary: true, scope: {} as never, projects: store as never }
}

beforeEach(() => {
  h.posts.length = 0
  useToastStore.setState({ toasts: [] })
})

describe('planning a clicked terminal URL', () => {
  it('web links stay web links', () => {
    for (const u of ['https://k2.dev/x', 'http://localhost:3000', 'mailto:a@b.co']) {
      expect(planTerminalUrl(u, ROOTS)).toEqual({ kind: 'web' })
    }
  })

  it('file:// inside a workspace (or a worktree) opens in the viewer', () => {
    expect(planTerminalUrl('file:///Users/me/ws/src/a%20b.ts', ROOTS)).toEqual({ kind: 'file', path: '/Users/me/ws/src/a b.ts' })
    expect(planTerminalUrl('file://localhost/Users/me/ws-wt/feature/x.md', ROOTS)).toEqual({
      kind: 'file',
      path: '/Users/me/ws-wt/feature/x.md',
    })
    expect(planTerminalUrl('file:///C:/Users/me/proj/README.md', ROOTS)).toEqual({ kind: 'file', path: 'C:/Users/me/proj/README.md' })
    expect(planTerminalUrl('file:///c:/users/ME/proj/x.txt', ['C:\\Users\\me\\proj'])).toEqual({
      kind: 'file',
      path: 'c:/users/ME/proj/x.txt',
    })
  })

  it('refuses file:// outside every workspace, a UNC host, .. steps, and other schemes', () => {
    for (const u of [
      'file:///etc/passwd',
      'file:///Users/me/wsx/a.txt',
      'file:///Users/me/ws',
      'file:///Users/me/ws/../secret.txt',
      'file://server/share/x.txt',
      'ssh://host',
      'ftp://example.com/x',
      'gemini://x.y',
      'javascript:alert(1)',
      'not a url',
    ]) {
      expect([u, planTerminalUrl(u, ROOTS).kind]).toEqual([u, 'refused'])
    }
  })

  it('helpers', () => {
    expect(fileUrlPath('file:///a/b')).toBe('/a/b')
    expect(fileUrlPath('file://host/a')).toBeNull()
    expect(fileUrlPath('https://x/a')).toBeNull()
    expect(insideAnyRoot('/Users/me/ws/a', ['/Users/me/ws/'])).toBe(true)
    expect(insideAnyRoot('/Users/me/ws/./a', ['/Users/me/ws'])).toBe(false)
    expect(terminalWorkspaceRoots(room().projects.getState().projects)).toEqual(ROOTS)
  })
})

describe('openTerminalUrl', () => {
  it('a file:// link in a workspace opens in K2’s viewer, never through the opener', () => {
    const opened: string[] = []
    openTerminalUrl(room(), 'file:///Users/me/ws/notes.md', (p) => opened.push(p))
    expect(opened).toEqual(['/Users/me/ws/notes.md'])
    expect(h.posts).toEqual([])
    expect(useToastStore.getState().toasts).toEqual([])
  })

  it('a refused link shows the toast and opens nothing', () => {
    const opened: string[] = []
    for (const u of ['file:///etc/hosts', 'ssh://host']) openTerminalUrl(room(), u, (p) => opened.push(p))
    expect(opened).toEqual([])
    expect(h.posts).toEqual([])
    const toasts = useToastStore.getState().toasts.map((t) => t.message)
    expect(toasts).toEqual([TERMINAL_LINK_REFUSED, TERMINAL_LINK_REFUSED])
  })

  it('a web link goes to the daemon opener as before', async () => {
    openTerminalUrl(room(), 'https://k2.dev/docs', () => {
      throw new Error('never a file')
    })
    await vi.waitFor(() => expect(h.posts).toEqual([{ route: 'fs/open-external', body: { target: 'https://k2.dev/docs' } }]))
  })
})

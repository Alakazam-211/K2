// @vitest-environment jsdom
// Thread file links against a server without `POST /cli/fs/resolve-links`
// (0.46.0). 0.46.1 RC smoke (Rosson 2026-10-18): the RC window showed this
// Mac's Threads (rosson.k2.dev, daemon 0.46.0) and none of the paths in a
// message opened, because the resolve call 405'd and the resolver dropped
// the whole Thread to plain text. The real transport now answers from the
// routes 0.46.0 has (workspace/resolve, fs/info, fs/read-dir), and a click
// goes all the way to a file tab in the Thread's workspace at its line.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'

const ROOT = '/Users/z3thon/DevProjects/Alakazam Labs/K2'
const HOME = '/Users/z3thon'

/** The 0.46.0 server's folders (name → is a folder). */
const TREE: Record<string, Record<string, boolean>> = {
  [ROOT]: { '.k2': true, crates: true, 'WHATS_NEW.md': false },
  [`${ROOT}/.k2`]: { prds: true },
  [`${ROOT}/.k2/prds`]: { 'mockup-projects-chatter-v1.html': false },
  [`${ROOT}/crates`]: { 'k2-daemon': true },
  [`${ROOT}/crates/k2-daemon`]: { src: true },
  [`${ROOT}/crates/k2-daemon/src`]: { 'push_routes.rs': false },
  '/etc': { hosts: false },
}

const calls = vi.hoisted(() => ({ get: [] as Array<{ route: string; params?: Record<string, unknown> }>, post: [] as string[] }))

vi.mock('@/lib/daemon-cli', () => ({
  // 0.46.0's top-level method guard: a POST to a route it does not route.
  daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
    calls.post.push(route)
    throw new DaemonHttpError('method not allowed for this route', 405, null)
  }),
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
    calls.get.push({ route, params })
    if (route === 'workspace/resolve') {
      if (params?.q !== 'k2') throw new DaemonHttpError(`workspace not found: ${String(params?.q)}`, 400, null)
      return { path: ROOT }
    }
    if (route === 'fs/info') return { home: HOME, separator: '/', os: 'macos' }
    if (route === 'fs/read-dir') {
      const dir = TREE[String(params?.path)]
      if (!dir) throw new DaemonHttpError('No such file or directory', 400, null)
      return Object.entries(dir).map(([name, isDirectory]) => ({ name, isDirectory, path: `${String(params?.path)}/${name}` }))
    }
    throw new Error(`unexpected GET ${route}`)
  }),
}))

const activateTab = vi.fn()
vi.mock('@/stores/panels', () => ({ usePanelsStore: { getState: () => ({ activateTab }) } }))
const projectsState = vi.hoisted(() => ({
  projects: [{ id: 'p-k2', path: '/Users/z3thon/DevProjects/Alakazam Labs/K2', workspaces: [{ id: 'w' }] }],
  activeProjectId: 'p-k2',
  setActiveProject: () => {},
}))

import { ChatMessageBody } from './ChatMessage'
import { FileLinkProvider, scopeFileLinkTransport, type FileLinkOpen } from './file-links'
import { openThreadFileLink } from '@/lib/thread-file-link-open'
import { takeFileLine } from '@/lib/file-open-requests'
import { isMissingResolveLinksRoute, legacyResolveLinks } from '@/lib/file-link-legacy'
import { registerPrimaryRoomProjects, type PrimaryProjectsStore } from '@/stores/room'

// The window's projects store, as the primary room reads it.
registerPrimaryRoomProjects({
  getState: () => projectsState,
  getInitialState: () => projectsState,
  subscribe: () => () => {},
} as unknown as PrimaryProjectsStore)

const SCOPE = { hostKey: 'host:rosson.k2.dev' } as unknown as ServerScope

/** The message K2 sent Rosson (inline code, a :line, a bare path with a
 *  space in it, and a path outside the workspace). */
const MESSAGE = [
  'Mockup: `.k2/prds/mockup-projects-chatter-v1.html`',
  'Route: `crates/k2-daemon/src/push_routes.rs:40`',
  `Notes: ${ROOT}/WHATS_NEW.md`,
  'Outside: `/etc/hosts`',
].join('\n')

async function flush(): Promise<void> {
  for (let i = 0; i < 20; i++) await act(async () => {})
}

beforeEach(() => {
  calls.get.length = 0
  calls.post.length = 0
  activateTab.mockClear()
})
afterEach(() => cleanup())

describe('Thread file links on a 0.46.0 server', () => {
  it('workspace paths become links (inline code, :line, a path with spaces); outside stays plain', async () => {
    const open = vi.fn<FileLinkOpen>()
    const { container } = render(
      <FileLinkProvider workspace="k2" transport={scopeFileLinkTransport(SCOPE)} open={open}>
        <ChatMessageBody text={MESSAGE} />
      </FileLinkProvider>,
    )
    await flush()
    const links = [...container.querySelectorAll('[role="link"]')]
    expect(links.map((l) => l.textContent)).toEqual([
      '.k2/prds/mockup-projects-chatter-v1.html',
      'crates/k2-daemon/src/push_routes.rs:40',
      `${ROOT}/WHATS_NEW.md`,
    ])
    expect(container.textContent).toContain('/etc/hosts')
    // The new route was tried once; after its 405 the transport stays on
    // the older routes, and never lists anything outside the workspace.
    expect(calls.post).toEqual(['fs/resolve-links'])
    expect(calls.get.some((c) => c.route === 'fs/read-dir' && c.params?.path === '/etc')).toBe(false)
    expect(calls.get.filter((c) => c.route === 'fs/read-dir').every((c) => c.params?.show_hidden === '1')).toBe(true)

    fireEvent.click(links[1])
    expect(open).toHaveBeenCalledTimes(1)
    const [answer, line, col] = open.mock.calls[0]
    expect(answer).toMatchObject({
      linkable: true,
      kind: 'file',
      rel: 'crates/k2-daemon/src/push_routes.rs',
      path: `${ROOT}/crates/k2-daemon/src/push_routes.rs`,
      root: ROOT,
    })
    expect(line).toBe(40)
    expect(col).toBeUndefined()
  })

  it('a click opens a file tab in the Thread\'s room at the line', async () => {
    const openFileAsTab = vi.fn()
    const room = { scope: SCOPE, tabs: { getState: () => ({ openFileAsTab }) }, isPrimary: true } as never
    const open: FileLinkOpen = (link, line, col) => openThreadFileLink(room, link, line, col)
    const { container } = render(
      <FileLinkProvider workspace="k2" transport={scopeFileLinkTransport(SCOPE)} open={open}>
        <ChatMessageBody text={MESSAGE} />
      </FileLinkProvider>,
    )
    await flush()
    const links = [...container.querySelectorAll('[role="link"]')]
    fireEvent.click(links[1])
    expect(openFileAsTab).toHaveBeenCalledWith(`${ROOT}/crates/k2-daemon/src/push_routes.rs`)
    expect(takeFileLine('host:rosson.k2.dev', `${ROOT}/crates/k2-daemon/src/push_routes.rs`)).toEqual({ line: 40 })
    fireEvent.keyDown(links[0], { key: 'Enter' })
    expect(openFileAsTab).toHaveBeenLastCalledWith(`${ROOT}/.k2/prds/mockup-projects-chatter-v1.html`)
  })

  it('an unknown workspace leaves every path plain (no link that does nothing)', async () => {
    const open = vi.fn<FileLinkOpen>()
    const { container } = render(
      <FileLinkProvider workspace="imported-thread" transport={scopeFileLinkTransport(SCOPE)} open={open}>
        <ChatMessageBody text={MESSAGE} />
      </FileLinkProvider>,
    )
    await flush()
    expect(container.querySelectorAll('[role="link"]').length).toBe(0)
  })
})

describe('legacyResolveLinks', () => {
  const deps = {
    resolveWorkspace: async (ws: string) => {
      if (ws !== 'k2') throw new Error('nope')
      return ROOT
    },
    home: async () => HOME,
    readDir: async (path: string) => {
      const dir = TREE[path]
      if (!dir) throw new Error('missing')
      return Object.entries(dir).map(([name, isDirectory]) => ({ name, isDirectory }))
    },
  }

  it('answers like the 0.46.1 route: inside → file/folder with rel + path; anything else refused bare', async () => {
    const res = await legacyResolveLinks(deps, 'k2/2', [
      'crates/k2-daemon/src/push_routes.rs',
      `${ROOT}/WHATS_NEW.md`,
      '~/DevProjects/Alakazam Labs/K2/.k2/prds',
      'crates/',
      '/etc/hosts',
      '../outside/a.ts',
      'crates/../../K2/WHATS_NEW.md',
      'src/missing.ts',
      '~bob/x',
    ])
    expect(res.root).toBe(ROOT)
    expect(res.prefixes).toEqual([ROOT, '~/DevProjects/Alakazam Labs/K2'])
    expect(res.links).toEqual([
      { input: 'crates/k2-daemon/src/push_routes.rs', linkable: true, kind: 'file', rel: 'crates/k2-daemon/src/push_routes.rs', path: `${ROOT}/crates/k2-daemon/src/push_routes.rs` },
      { input: `${ROOT}/WHATS_NEW.md`, linkable: true, kind: 'file', rel: 'WHATS_NEW.md', path: `${ROOT}/WHATS_NEW.md` },
      { input: '~/DevProjects/Alakazam Labs/K2/.k2/prds', linkable: true, kind: 'directory', rel: '.k2/prds', path: `${ROOT}/.k2/prds` },
      { input: 'crates/', linkable: true, kind: 'directory', rel: 'crates', path: `${ROOT}/crates` },
      { input: '/etc/hosts', linkable: false },
      { input: '../outside/a.ts', linkable: false },
      // Walks out and back in: the folded path is inside, so it links.
      { input: 'crates/../../K2/WHATS_NEW.md', linkable: true, kind: 'file', rel: 'WHATS_NEW.md', path: `${ROOT}/WHATS_NEW.md` },
      { input: 'src/missing.ts', linkable: false },
      { input: '~bob/x', linkable: false },
    ])
  })

  it('an unknown workspace throws (the resolver then leaves the Thread plain)', async () => {
    await expect(legacyResolveLinks(deps, 'who', ['a/b.ts'])).rejects.toThrow()
  })

  it('only a missing route switches to the older routes', () => {
    expect(isMissingResolveLinksRoute(new DaemonHttpError('method not allowed for this route', 405, null))).toBe(true)
    expect(isMissingResolveLinksRoute(new DaemonHttpError('route_unclassified', 404, null))).toBe(true)
    // The 0.46.1 route refusing this Thread's workspace is not "missing".
    expect(isMissingResolveLinksRoute(new DaemonHttpError('workspace not found', 404, null))).toBe(false)
    expect(isMissingResolveLinksRoute(new Error('network'))).toBe(false)
  })
})

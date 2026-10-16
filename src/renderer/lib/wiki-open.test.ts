// @vitest-environment jsdom
//
// The top bar's Wiki tab (0.46.0, Rosson 2026-10-16): which wiki it opens.
// Agents: the room's workspace. Home: the selected agent's workspace when it
// is on this window's server. Otherwise (Home on another server's agent,
// nothing selected, other pages): the last wiki viewed in this window, else
// the first workspace in the nav order.

import { afterEach, describe, expect, it } from 'vitest'
import {
  currentWikiDefaultInput,
  defaultWikiPath,
  openWikiForView,
  wikiNavOrder,
  type WikiDefaultInput,
  type WikiWorkspaceRef,
} from './wiki-open'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectsStore } from '@/stores/projects'
import { useHomesStore } from '@/stores/homes'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useConnectHostStore } from '@/stores/connect-host'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { activeHomeHostKey } from '@/lib/home-address'

const HERE = 'rosson.k2.dev'

const ws = (id: string, extra: Partial<WikiWorkspaceRef> = {}): WikiWorkspaceRef => ({
  id,
  path: `/w/${id}`,
  name: id,
  handle: id,
  pinned: 0,
  focusGroupId: null,
  workspaces: [],
  ...extra,
})

const PROJECTS = [
  ws('alpha', { focusGroupId: 'g2' }),
  ws('bravo', { focusGroupId: 'g1' }),
  ws('cortana', { pinned: 1, focusGroupId: 'g2' }),
  ws('delta', { workspaces: [{ worktreePath: '/w/delta/.worktrees/fix' }] }),
]

function input(over: Partial<WikiDefaultInput> = {}): WikiDefaultInput {
  return {
    page: 'agents',
    projects: PROJECTS,
    activeProjectId: null,
    homeRows: [],
    remoteRoomShown: false,
    connectedKey: HERE,
    activeFocusGroupId: null,
    lastWikiPath: null,
    ...over,
  }
}

describe('defaultWikiPath', () => {
  it('Agents: the room\'s active workspace', () => {
    expect(defaultWikiPath(input({ page: 'agents', activeProjectId: 'bravo' }))).toBe('/w/bravo')
  })

  it('Home: the selected agent when it is a row on this window\'s server', () => {
    const homeRows = [
      { address: `alpha::${HERE}`, workspaceId: 'alpha' },
      { address: `bravo::${HERE}`, workspaceId: 'bravo' },
    ]
    expect(defaultWikiPath(input({ page: 'home', activeProjectId: 'bravo', homeRows }))).toBe('/w/bravo')
  })

  it('Home: a remote agent on screen → the last wiki viewed, else the first in nav order', () => {
    const homeRows = [{ address: `bravo::${HERE}`, workspaceId: 'bravo' }, { address: 'zed::other.k2.dev', workspaceId: null }]
    const remote = input({ page: 'home', activeProjectId: 'bravo', homeRows, remoteRoomShown: true })
    expect(defaultWikiPath({ ...remote, lastWikiPath: '/w/alpha' })).toBe('/w/alpha')
    // No last wiki: Pinned first.
    expect(defaultWikiPath(remote)).toBe('/w/cortana')
  })

  it('Home: the active workspace is not a row of this Home → treated as nothing selected', () => {
    const homeRows = [{ address: `alpha::${HERE}`, workspaceId: 'alpha' }]
    expect(defaultWikiPath(input({ page: 'home', activeProjectId: 'bravo', homeRows }))).toBe('/w/cortana')
  })

  it('Home: a row with the same handle on another server is not this server\'s', () => {
    const homeRows = [{ address: 'bravo::other.k2.dev', workspaceId: 'bravo' }]
    expect(defaultWikiPath(input({ page: 'home', activeProjectId: 'bravo', homeRows, lastWikiPath: '/w/delta' }))).toBe('/w/delta')
  })

  it('nothing selected: last wiki (a worktree path counts), else first in nav order', () => {
    expect(defaultWikiPath(input({ lastWikiPath: '/w/delta/.worktrees/fix' }))).toBe('/w/delta/.worktrees/fix')
    expect(defaultWikiPath(input())).toBe('/w/cortana')
  })

  it('a last wiki from another server (not in this list) is skipped', () => {
    expect(defaultWikiPath(input({ lastWikiPath: '/srv/elsewhere' }))).toBe('/w/cortana')
  })

  it('Projects / Tickets pages: same fallback as nothing selected', () => {
    expect(defaultWikiPath(input({ page: 'projects', activeProjectId: 'bravo' }))).toBe('/w/cortana')
    expect(defaultWikiPath(input({ page: 'feedback', activeProjectId: 'bravo', lastWikiPath: '/w/alpha' }))).toBe('/w/alpha')
  })

  it('no workspaces at all → null', () => {
    expect(defaultWikiPath(input({ projects: [] }))).toBeNull()
  })
})

describe('wikiNavOrder', () => {
  it('Pinned, then the focus group\'s list (ungrouped included), then the rest', () => {
    expect(wikiNavOrder(PROJECTS, null).map((p) => p.id)).toEqual(['cortana', 'alpha', 'bravo', 'delta'])
    expect(wikiNavOrder(PROJECTS, 'g1').map((p) => p.id)).toEqual(['cortana', 'bravo', 'delta', 'alpha'])
  })
})

describe('openWikiForView (the window\'s stores)', () => {
  const projects = PROJECTS.map((p) => ({ ...p, color: '#888', iconUrl: null }))
  afterEach(() => {
    usePageViewStore.getState().setPage('agents')
    useHomeRoomsStore.setState({ shown: null })
  })

  it('Agents: opens the room\'s workspace wiki and remembers Agents', () => {
    useProjectsStore.setState({ projects: projects as never, activeProjectId: 'alpha' })
    usePageViewStore.getState().setPage('agents')
    openWikiForView()
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiProjectPath, s.wikiReturnPage, s.lastWikiPath]).toEqual(['wiki', '/w/alpha', 'agents', '/w/alpha'])
  })

  it('Home: reads the selected Home, the shown room and the connected server', () => {
    const key = activeHomeHostKey(useConnectHostStore.getState().activeHost)
    useProjectsStore.setState({ projects: projects as never, activeProjectId: 'bravo' })
    const home = { id: 'h1', name: 'Mine', rows: [{ address: `bravo::${key}`, workspaceId: 'bravo', label: 'bravo' }] }
    useHomesStore.setState({ homes: [home], selectedId: 'h1' })
    useFocusGroupsStore.setState({ focusGroupsEnabled: false })
    usePageViewStore.getState().setPage('home')
    expect(currentWikiDefaultInput().connectedKey).toBe(key)
    openWikiForView()
    expect([usePageViewStore.getState().wikiProjectPath, usePageViewStore.getState().wikiReturnPage]).toEqual(['/w/bravo', 'home'])

    // A remote room on screen: last wiki viewed (bravo, just above).
    usePageViewStore.getState().closeWiki()
    useProjectsStore.setState({ activeProjectId: 'alpha' })
    useHomesStore.setState({ homes: [{ ...home, rows: [...home.rows, { address: `alpha::${key}`, workspaceId: 'alpha', label: 'alpha' }] }] })
    useHomeRoomsStore.setState({ shown: 'zed::other.k2.dev' })
    expect(usePageViewStore.getState().page).toBe('home')
    openWikiForView()
    expect(usePageViewStore.getState().wikiProjectPath).toBe('/w/bravo')
  })
})

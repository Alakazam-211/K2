// @vitest-environment jsdom
// prd-projects-threads-v1 §3.2: an agent tile's view, from the project
// default, with a personal switch that the default resets.
import { beforeEach, describe, expect, it } from 'vitest'
import {
  TILE_DEFAULT_VIEW,
  TILE_VIEWS,
  pickTileView,
  projectDefaultView,
  resolveTileView,
  tileViewKey,
} from './tile-view'
import { useProjectGroupsStore } from '@/stores/project-groups'

describe('projectDefaultView', () => {
  it('reads the three project views as they are', () => {
    expect(projectDefaultView('terminal')).toBe('terminal')
    expect(projectDefaultView('thread')).toBe('thread')
    expect(projectDefaultView('chatter')).toBe('chatter')
  })

  it('falls back to Thread when the field is missing (a daemon from before it)', () => {
    expect(TILE_DEFAULT_VIEW).toBe('thread')
    expect(projectDefaultView(undefined)).toBe('thread')
    expect(projectDefaultView(null)).toBe('thread')
  })

  it("ignores a value it doesn't know: Split, Heartbeats (personal only), junk", () => {
    expect(projectDefaultView('split')).toBe('thread')
    expect(projectDefaultView('heartbeats')).toBe('thread')
    expect(projectDefaultView('Terminal')).toBe('thread')
    expect(projectDefaultView(3)).toBe('thread')
  })
})

describe('the tile views', () => {
  it('are Terminal, Thread, Chatter and Heartbeats, never Split', () => {
    expect(TILE_VIEWS).toEqual(['terminal', 'thread', 'chatter', 'heartbeats'])
    expect((TILE_VIEWS as readonly string[]).includes('split')).toBe(false)
  })
})

describe('resolveTileView', () => {
  it('shows the project default when the tile was never switched', () => {
    expect(resolveTileView(undefined, 'thread')).toEqual({ view: 'thread', stale: false })
    expect(resolveTileView(undefined, 'terminal')).toEqual({ view: 'terminal', stale: false })
  })

  it('shows a switch made against the current default', () => {
    expect(resolveTileView({ view: 'terminal', against: 'thread' }, 'thread')).toEqual({ view: 'terminal', stale: false })
    expect(resolveTileView({ view: 'heartbeats', against: 'chatter' }, 'chatter')).toEqual({ view: 'heartbeats', stale: false })
  })

  it('drops a switch once the default moves, and shows the new default', () => {
    expect(resolveTileView({ view: 'terminal', against: 'thread' }, 'chatter')).toEqual({ view: 'chatter', stale: true })
    // Even when the new default equals the switched view.
    expect(resolveTileView({ view: 'terminal', against: 'thread' }, 'terminal')).toEqual({ view: 'terminal', stale: true })
  })
})

describe('pickTileView', () => {
  it('stores the pick with the default it was made against', () => {
    expect(pickTileView('terminal', 'thread')).toEqual({ view: 'terminal', against: 'thread' })
  })
  it('stores nothing for the default itself', () => {
    expect(pickTileView('thread', 'thread')).toBeNull()
  })
})

describe('tileViewKey', () => {
  it('is per server, per project, per agent', () => {
    expect(tileViewKey('local', 'g1', 'ws1')).toBe('local|g1|ws1')
    expect(tileViewKey('h:box:7', 'g1', 'ws1')).not.toBe(tileViewKey('local', 'g1', 'ws1'))
    expect(tileViewKey('local', 'g2', 'ws1')).not.toBe(tileViewKey('local', 'g1', 'ws1'))
  })
})

describe('useProjectGroupsStore tile views (in memory only)', () => {
  const key = tileViewKey('local', 'g1', 'ws1')
  beforeEach(() => useProjectGroupsStore.setState({ tileViews: {} }))

  it('a switch is stored, and picking the default again clears it', () => {
    const s = useProjectGroupsStore.getState()
    s.setTileView(key, 'terminal', 'thread')
    expect(useProjectGroupsStore.getState().tileViews[key]).toEqual({ view: 'terminal', against: 'thread' })
    s.setTileView(key, 'thread', 'thread')
    expect(useProjectGroupsStore.getState().tileViews[key]).toBeUndefined()
  })

  it('the default change resets a switched tile', () => {
    useProjectGroupsStore.getState().setTileView(key, 'chatter', 'thread')
    const entry = useProjectGroupsStore.getState().tileViews[key]
    const { view, stale } = resolveTileView(entry, 'terminal')
    expect(view).toBe('terminal')
    expect(stale).toBe(true)
    useProjectGroupsStore.getState().dropTileView(key)
    expect(useProjectGroupsStore.getState().tileViews).toEqual({})
  })

  it('never writes the switch to localStorage', () => {
    const before = JSON.stringify({ ...localStorage })
    useProjectGroupsStore.getState().setTileView(key, 'terminal', 'thread')
    expect(JSON.stringify({ ...localStorage })).toBe(before)
  })
})

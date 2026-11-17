// "Move to group…" on a Home row's right-click menu: a submenu of the
// person's other groups (never the one the row is in), a group that already
// has the row is off with "Already in it", and "New group" at the end makes
// the next "Group N" and moves the row into it. With one group the item is
// shown but off, saying why. The move is one change to the Homes store, so
// Home sync sends it like any other edit: the row leaves one group and
// lands in the other.

import { beforeEach, describe, expect, it } from 'vitest'
import { useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import { diffLocal, emptySyncState, type SyncState, type WireRow } from '@/lib/homes-sync/engine'
import type { ContextMenuItemDef } from '@/stores/context-menu'
import { MOVE_TO_GROUP_NEW_ID, moveToGroupItem, moveToGroupTargetId, runMoveToGroup } from './move-to-group'

const agent: HomeRow = { address: 'cortana::local', workspaceId: 'pl', label: 'Cortana' }
const person: HomeRow = { kind: 'person', address: 'k2:connect:a::local', workspaceId: null, label: 'Alex' }

function group(id: string, name: string, rows: HomeRow[] = [], extra: Record<string, unknown> = {}): Home {
  return { id, name, rows, ...extra } as Home
}

function seed(homes: Home[], selectedId = homes[0].id): void {
  useHomesStore.setState({ homes, selectedId })
}

function home(id: string): Home {
  const h = useHomesStore.getState().homes.find((x) => x.id === id)
  if (!h) throw new Error(`no group ${id}`)
  return h
}

function sub(item: ContextMenuItemDef): ContextMenuItemDef[] {
  if (!item.submenu) throw new Error(`${item.label} has no submenu`)
  return item.submenu
}

const ctx = { toWire: (a: string) => a, fromWire: (a: string) => a, deviceId: 'dev-a', now: 1000 }
function merged(): SyncState {
  return { ...emptySyncState('dev-a'), merged: true, serverId: 'aaaaaaaaaaaaaaaa', cursor: 0 }
}

beforeEach(() => {
  seed([group('g1', 'Group 1', [agent]), group('g2', 'Work'), group('g3', 'Ops', [agent])])
})

describe('Move to group… menu item', () => {
  it('lists every other group, never the one the row is in', () => {
    const item = moveToGroupItem(useHomesStore.getState().homes, home('g1'), agent)
    expect(item.label).toBe('Move to group…')
    expect(item.enabled).not.toBe(false)
    const rows = sub(item).filter((r) => r.type !== 'separator')
    expect(rows.map((r) => r.label)).toEqual(['Work', 'Ops', 'New group'])
    expect(rows.map((r) => r.id)).not.toContain(moveToGroupTargetId('g1'))
  })

  it('a group that already has the row is off, with "Already in it"', () => {
    const rows = sub(moveToGroupItem(useHomesStore.getState().homes, home('g1'), agent))
    const ops = rows.find((r) => r.id === moveToGroupTargetId('g3'))
    expect(ops).toMatchObject({ label: 'Ops', enabled: false, hint: 'Already in it' })
    const work = rows.find((r) => r.id === moveToGroupTargetId('g2'))
    expect(work?.enabled).not.toBe(false)
    expect(work?.hint).toBeUndefined()
  })

  it('with only one group it is shown but off, saying why', () => {
    seed([group('g1', 'Group 1', [agent])])
    const item = moveToGroupItem(useHomesStore.getState().homes, home('g1'), agent)
    expect(item).toMatchObject({ label: 'Move to group…', enabled: false, hint: 'Make another group first.' })
    expect(item.submenu).toBeUndefined()
  })
})

describe('Moving a row', () => {
  it('lands in the target group, leaves its own, and Home sync sends both', () => {
    const before = diffLocal(useHomesStore.getState().homes, merged(), ctx).state
    let commits = 0
    const unsub = useHomesStore.subscribe((s, prev) => {
      if (s.homes !== prev.homes) commits++
    })
    expect(runMoveToGroup(moveToGroupTargetId('g2'), home('g1'), agent)).toBe(true)
    unsub()
    expect(commits).toBe(1)
    expect(home('g1').rows).toEqual([])
    expect(home('g2').rows.map((r) => r.address)).toEqual(['cortana::local'])
    // The window stays on the group it was on.
    expect(useHomesStore.getState().selectedId).toBe('g1')

    const { changes } = diffLocal(useHomesStore.getState().homes, before, { ...ctx, now: 2000 })
    const rows = changes.filter((c): c is WireRow => c.kind !== 'home')
    expect(rows.find((r) => r.homeId === 'g1')).toMatchObject({ address: 'cortana::local', deleted: true })
    expect(rows.find((r) => r.homeId === 'g2')).toMatchObject({ address: 'cortana::local' })
    expect(rows.find((r) => r.homeId === 'g2')?.deleted).toBeUndefined()
  })

  it('keeps its last send but not its pin (pins belong to a group)', () => {
    seed([group('g1', 'Group 1', [{ ...agent, pinOrder: 'a0', lastMessagedAt: 55 }]), group('g2', 'Work')])
    expect(runMoveToGroup(moveToGroupTargetId('g2'), home('g1'), home('g1').rows[0])).toBe(true)
    expect(home('g2').rows[0]).toEqual({ address: 'cortana::local', workspaceId: 'pl', label: 'Cortana', lastMessagedAt: 55 })
  })

  it('never moves into a group that already has it', () => {
    expect(useHomesStore.getState().moveRowToHome('g1', 'g3', 'cortana::local')).toBe(false)
    expect(home('g1').rows.map((r) => r.address)).toEqual(['cortana::local'])
    expect(home('g3').rows.map((r) => r.address)).toEqual(['cortana::local'])
  })

  it('a built-in People group is a target like any other', () => {
    seed([group('g1', 'Group 1', [person]), group('people', 'People', [], { builtin: 'people' })])
    const rows = sub(moveToGroupItem(useHomesStore.getState().homes, home('g1'), person))
    expect(rows.find((r) => r.id === moveToGroupTargetId('people'))).toMatchObject({ label: 'People' })
    expect(runMoveToGroup(moveToGroupTargetId('people'), home('g1'), person)).toBe(true)
    expect(home('people').rows.map((r) => r.address)).toEqual(['k2:connect:a::local'])
    expect(home('g1').rows).toEqual([])
  })

  it('"New group" makes the next "Group N" and moves the row into it', () => {
    const rows = sub(moveToGroupItem(useHomesStore.getState().homes, home('g1'), agent))
    expect(rows[rows.length - 1]).toMatchObject({ id: MOVE_TO_GROUP_NEW_ID, label: 'New group' })
    expect(runMoveToGroup(MOVE_TO_GROUP_NEW_ID, home('g1'), agent)).toBe(true)
    const homes = useHomesStore.getState().homes
    const made = homes[homes.length - 1]
    expect(made.name).toBe('Group 2')
    expect(made.rows.map((r) => r.address)).toEqual(['cortana::local'])
    expect(home('g1').rows).toEqual([])
  })

  it('any other pick does nothing', () => {
    expect(runMoveToGroup(null, home('g1'), agent)).toBe(false)
    expect(runMoveToGroup('home-pin', home('g1'), agent)).toBe(false)
    expect(home('g1').rows.map((r) => r.address)).toEqual(['cortana::local'])
  })
})

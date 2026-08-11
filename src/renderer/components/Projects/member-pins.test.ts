import { describe, it, expect, beforeEach } from 'vitest'
import {
  loadPinnedMemberIds,
  loadPinnedResourceKeys,
  memberPinsStorageKey,
  resourceDocKey,
  savePinnedMemberIds,
  sortMembersPinnedFirst,
  sortResourcesPinnedThenAlpha,
  togglePinnedMember,
  togglePinnedResource,
} from './member-pins'
import type { ProjectGroupHtmlDoc, ProjectGroupMemberInfo } from './projects-api'

function mem(id: string): ProjectGroupMemberInfo {
  return {
    workspaceId: id,
    name: id,
    path: `/ws/${id}`,
    agentName: id,
    createdAt: 0,
  }
}

// Minimal localStorage for node vitest (no happy-dom by default).
const store = new Map<string, string>()
const ls = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => {
    store.set(k, v)
  },
  removeItem: (k: string) => {
    store.delete(k)
  },
  clear: () => store.clear(),
}
// @ts-expect-error test shim
globalThis.localStorage = ls

describe('member-pins', () => {
  beforeEach(() => {
    store.clear()
  })

  it('storage key is per group', () => {
    expect(memberPinsStorageKey('g1')).toBe('k2:project-nav:pinned-members:g1')
  })

  it('round-trips pin list', () => {
    savePinnedMemberIds('g1', ['a', 'b'])
    expect(loadPinnedMemberIds('g1')).toEqual(['a', 'b'])
  })

  it('toggle pins then unpins', () => {
    expect(togglePinnedMember('g1', 'w1')).toEqual(['w1'])
    expect(togglePinnedMember('g1', 'w2')).toEqual(['w1', 'w2'])
    expect(togglePinnedMember('g1', 'w1')).toEqual(['w2'])
  })

  it('sorts pinned first preserving pin order and rest order', () => {
    const members = [mem('c'), mem('a'), mem('b'), mem('d')]
    const { sorted, pinnedCount } = sortMembersPinnedFirst(members, ['b', 'a', 'gone'])
    expect(pinnedCount).toBe(2)
    expect(sorted.map((m) => m.workspaceId)).toEqual(['b', 'a', 'c', 'd'])
  })
})

function doc(name: string, path?: string): ProjectGroupHtmlDoc {
  return {
    workspaceId: 'w1',
    workspaceName: 'ws',
    agentName: 'agent',
    filePath: path ?? `/docs/${name}`,
    fileName: name,
  }
}

describe('resource pins', () => {
  beforeEach(() => {
    store.clear()
  })

  it('toggles resource pin keys', () => {
    const d = doc('z.html')
    expect(togglePinnedResource('g1', d)).toEqual([resourceDocKey(d)])
    expect(loadPinnedResourceKeys('g1')).toEqual([resourceDocKey(d)])
    expect(togglePinnedResource('g1', d)).toEqual([])
  })

  it('sorts pinned first then alphabetical by fileName', () => {
    const docs = [doc('zeta.html'), doc('alpha.html'), doc('beta.html'), doc('gamma.html')]
    const pinned = [resourceDocKey(docs[2]), resourceDocKey(docs[0])] // beta, zeta
    const { sorted, pinnedCount } = sortResourcesPinnedThenAlpha(docs, pinned)
    expect(pinnedCount).toBe(2)
    expect(sorted.map((d) => d.fileName)).toEqual([
      'beta.html',
      'zeta.html',
      'alpha.html',
      'gamma.html',
    ])
  })
})

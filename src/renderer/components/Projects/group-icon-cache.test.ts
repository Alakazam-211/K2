// §6.7.7 — the project-group icon cache's invalidation semantics: keys
// are host-scoped (remoting never shows the wrong host's icons) and a
// groups-changed drop clears the group EVERYWHERE (a payload without a
// groupId clears all — correctness over thrift).

import { describe, it, expect, beforeEach } from 'vitest'
import {
  dropCachedGroupIcon,
  getCachedGroupIcon,
  setCachedGroupIcon,
} from './group-icon-cache'

const ICON = { found: true, dataUrl: 'data:image/png;base64,x' }
const MISS = { found: false, dataUrl: null }

describe('group-icon-cache', () => {
  beforeEach(() => {
    dropCachedGroupIcon() // clear all — the module map survives between tests
  })

  it('keys entries per host + group', () => {
    setCachedGroupIcon('local', 'g1', ICON)
    setCachedGroupIcon('remote:rpm', 'g1', MISS)
    expect(getCachedGroupIcon('local', 'g1')).toEqual(ICON)
    expect(getCachedGroupIcon('remote:rpm', 'g1')).toEqual(MISS)
    expect(getCachedGroupIcon('local', 'g2')).toBeUndefined()
  })

  it('drop by group clears that group across ALL hosts, others survive', () => {
    setCachedGroupIcon('local', 'g1', ICON)
    setCachedGroupIcon('remote:rpm', 'g1', ICON)
    setCachedGroupIcon('local', 'g2', MISS)
    dropCachedGroupIcon('g1')
    expect(getCachedGroupIcon('local', 'g1')).toBeUndefined()
    expect(getCachedGroupIcon('remote:rpm', 'g1')).toBeUndefined()
    expect(getCachedGroupIcon('local', 'g2')).toEqual(MISS)
  })

  it('drop without a groupId clears everything (malformed payload path)', () => {
    setCachedGroupIcon('local', 'g1', ICON)
    setCachedGroupIcon('local', 'g2', MISS)
    dropCachedGroupIcon(undefined)
    expect(getCachedGroupIcon('local', 'g1')).toBeUndefined()
    expect(getCachedGroupIcon('local', 'g2')).toBeUndefined()
  })

  it('never cross-matches a host suffix that merely CONTAINS the group id', () => {
    // Suffix matching is exact `:${groupId}` — a different group whose id
    // ends differently must survive.
    setCachedGroupIcon('local', 'group-11', ICON)
    setCachedGroupIcon('local', '1', MISS)
    dropCachedGroupIcon('1')
    expect(getCachedGroupIcon('local', 'group-11')).toEqual(ICON)
    expect(getCachedGroupIcon('local', '1')).toBeUndefined()
  })
})

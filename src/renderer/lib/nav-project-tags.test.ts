import { describe, it, expect } from 'vitest'
import {
  buildTagsByWorkspaceId,
  packNavTags,
  navTagsTooltip,
} from './nav-project-tags'

const groups = [
  { id: 'sales', name: 'Sales', color: '#f00' },
  { id: 'ops', name: 'Ops', color: null },
  { id: 'rpm', name: 'RPMAVS', color: '#0af' },
]

describe('buildTagsByWorkspaceId', () => {
  it('indexes each workspace to its groups, name-sorted', () => {
    const index = buildTagsByWorkspaceId(groups, {
      rpm: ['cortana'],
      sales: ['cortana', 'sarah'],
      ops: ['k2-dev-web'],
    })
    expect(index.cortana.map((t) => t.name)).toEqual(['RPMAVS', 'Sales'])
    expect(index.sarah.map((t) => t.id)).toEqual(['sales'])
    expect(index['k2-dev-web'][0].name).toBe('Ops')
    expect(index.nobody).toBeUndefined()
  })

  it('skips empty member ids and duplicate memberships', () => {
    const index = buildTagsByWorkspaceId(groups, {
      sales: ['cortana', '', 'cortana'],
    })
    expect(index.cortana).toHaveLength(1)
  })
})

describe('packNavTags', () => {
  const tags = groups.map((g) => ({ id: g.id, name: g.name, color: g.color }))

  it('keeps two or fewer in the row', () => {
    expect(packNavTags(tags.slice(0, 2)).overflow).toEqual([])
    expect(packNavTags(tags.slice(0, 1)).visible).toHaveLength(1)
  })

  it('collapses the rest into overflow for a +N chip', () => {
    const packed = packNavTags(tags)
    expect(packed.visible.map((t) => t.name)).toEqual(['Sales', 'Ops'])
    expect(packed.overflow.map((t) => t.name)).toEqual(['RPMAVS'])
  })
})

describe('navTagsTooltip', () => {
  it('lists every group name', () => {
    expect(navTagsTooltip([{ id: 'a', name: 'Sales', color: null }, { id: 'b', name: 'Ops', color: null }])).toBe(
      'Sales, Ops',
    )
  })
})

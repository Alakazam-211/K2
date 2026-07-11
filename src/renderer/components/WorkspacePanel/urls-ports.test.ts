// URLs drawer + settings — pure derivation logic (urls-ports.ts).

import { describe, it, expect } from 'vitest'
import {
  nestedPublicUrl,
  normalizeTargets,
  sortedTargets,
  unattributedCount,
  workspaceTargets,
  type SubdomainTargetInfo,
} from './urls-ports'

describe('nestedPublicUrl', () => {
  it('prefixes the label onto the tunnel public host', () => {
    expect(nestedPublicUrl('staging', 'rosson', 'https://rosson.k2.dev')).toBe(
      'https://staging.rosson.k2.dev',
    )
  })

  it('tolerates a trailing slash on the public URL', () => {
    expect(nestedPublicUrl('staging', '', 'https://rosson.k2.dev/')).toBe(
      'https://staging.rosson.k2.dev',
    )
  })

  it('falls back to primary label + k2.dev when no public URL', () => {
    expect(nestedPublicUrl('staging', 'rosson', null)).toBe(
      'https://staging.rosson.k2.dev',
    )
  })

  it('publicUrl wins over the primary fallback (host truth over label guess)', () => {
    // A daemon on a non-default host suffix must derive from the URL it
    // actually predicted, not the k2.dev fallback.
    expect(nestedPublicUrl('staging', 'rosson', 'https://rosson.custom.example')).toBe(
      'https://staging.rosson.custom.example',
    )
  })

  it('returns null when neither publicUrl nor primary is known (never fabricate)', () => {
    expect(nestedPublicUrl('staging', '', null)).toBeNull()
    expect(nestedPublicUrl('staging', '  ', '')).toBeNull()
  })

  it('returns null for a blank label', () => {
    expect(nestedPublicUrl('', 'rosson', 'https://rosson.k2.dev')).toBeNull()
    expect(nestedPublicUrl('   ', 'rosson', null)).toBeNull()
  })

  it('ignores a non-https publicUrl and uses the primary fallback', () => {
    expect(nestedPublicUrl('staging', 'rosson', 'http://insecure.example')).toBe(
      'https://staging.rosson.k2.dev',
    )
    expect(nestedPublicUrl('staging', '', 'http://insecure.example')).toBeNull()
  })
})

describe('normalizeTargets', () => {
  it('passes through the 0074 attributed object shape', () => {
    expect(
      normalizeTargets({
        staging: { target: 'localhost:3000', projectId: 'proj-1' },
        preview: { target: '127.0.0.1:8080', projectId: null },
      }),
    ).toEqual({
      staging: { target: 'localhost:3000', projectId: 'proj-1' },
      preview: { target: '127.0.0.1:8080', projectId: null },
    })
  })

  it('normalizes the pre-0074 bare-string shape as unattributed', () => {
    // An older remote daemon still broadcasts `label → "host:port"`.
    expect(normalizeTargets({ staging: 'localhost:3000' })).toEqual({
      staging: { target: 'localhost:3000', projectId: null },
    })
  })

  it('handles a mixed map (older event replayed over a newer snapshot)', () => {
    expect(
      normalizeTargets({
        old: 'localhost:4000',
        attributed: { target: 'localhost:3000', projectId: 'proj-1' },
      }),
    ).toEqual({
      old: { target: 'localhost:4000', projectId: null },
      attributed: { target: 'localhost:3000', projectId: 'proj-1' },
    })
  })

  it('drops junk entries instead of rendering broken rows', () => {
    expect(
      normalizeTargets({
        blank: '',
        noTarget: { projectId: 'proj-1' },
        numeric: 42,
        ok: { target: 'localhost:3000', projectId: 'proj-1' },
      }),
    ).toEqual({ ok: { target: 'localhost:3000', projectId: 'proj-1' } })
  })

  it('non-object wire values yield an empty map', () => {
    expect(normalizeTargets(null)).toEqual({})
    expect(normalizeTargets(undefined)).toEqual({})
    expect(normalizeTargets('nope')).toEqual({})
  })
})

const attributed: Record<string, SubdomainTargetInfo> = {
  mine: { target: 'localhost:3000', projectId: 'proj-1' },
  'mine-too': { target: 'localhost:3001', projectId: 'proj-1' },
  theirs: { target: 'localhost:4000', projectId: 'proj-2' },
  loose: { target: 'localhost:5000', projectId: null },
}

describe('workspaceTargets', () => {
  it('keeps only the rows attributed to the given project (the drawer filter)', () => {
    expect(Object.keys(workspaceTargets(attributed, 'proj-1')).sort()).toEqual([
      'mine',
      'mine-too',
    ])
    expect(workspaceTargets(attributed, 'proj-2')).toEqual({
      theirs: { target: 'localhost:4000', projectId: 'proj-2' },
    })
  })

  it('never surfaces unattributed rows in a workspace view', () => {
    expect(workspaceTargets(attributed, 'proj-1')).not.toHaveProperty('loose')
  })

  it('empty state: a workspace with no attributed URLs gets an empty map', () => {
    expect(workspaceTargets(attributed, 'proj-none')).toEqual({})
  })

  it('a blank projectId matches nothing (never leak server-wide rows)', () => {
    expect(workspaceTargets(attributed, '')).toEqual({})
  })
})

describe('unattributedCount (claim-hint counter)', () => {
  it('counts only the projectId-null rows', () => {
    expect(unattributedCount(attributed)).toBe(1)
  })

  it('zero when every row is attributed — no claim hint', () => {
    expect(
      unattributedCount({
        a: { target: 'x', projectId: 'p1' },
        b: { target: 'y', projectId: 'p2' },
      }),
    ).toBe(0)
    expect(unattributedCount({})).toBe(0)
  })
})

describe('sortedTargets', () => {
  it('sorts rows by label so the table never reshuffles across refreshes', () => {
    expect(
      sortedTargets({
        preview: { target: '127.0.0.1:8080', projectId: null },
        api: { target: 'localhost:4000', projectId: 'p1' },
        staging: { target: 'localhost:3000', projectId: null },
      }).map(([label]) => label),
    ).toEqual(['api', 'preview', 'staging'])
  })

  it('empty map yields an empty list', () => {
    expect(sortedTargets({})).toEqual([])
  })
})

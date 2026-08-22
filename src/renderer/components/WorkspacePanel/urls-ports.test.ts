// Published drawer + settings — pure derivation logic (urls-ports.ts).

import { describe, it, expect } from 'vitest'
import {
  PUBLISH_RUN_EXAMPLE,
  byoWorkspaceTargets,
  isLocalOnly,
  isServiceStoppable,
  nestedPublicUrl,
  normalizeTargets,
  parsePublishList,
  serviceListenLabel,
  servicePublicUrl,
  sortedServices,
  sortedTargets,
  unattributedCount,
  unattributedHint,
  workspaceTargets,
  type PublishedService,
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

function svc(over: Partial<PublishedService> & { name: string }): PublishedService {
  return {
    cmd: 'npm start',
    cwd: '/proj',
    port: 3000,
    expose: 'tunnel',
    desired: 'running',
    status: 'running',
    pid: 42,
    url: 'https://web.rosson.k2.dev',
    target: 'localhost:3000',
    error: null,
    lastExitCode: null,
    ...over,
  }
}

describe('parsePublishList', () => {
  it('reads the frozen { services: [...] } list shape', () => {
    expect(
      parsePublishList({
        services: [
          {
            name: 'web',
            cmd: 'npm start',
            cwd: '/app',
            port: 3000,
            expose: 'tunnel',
            desired: 'running',
            status: 'running',
            pid: 9,
            url: 'https://web.rosson.k2.dev',
            target: 'localhost:3000',
            error: null,
            lastExitCode: null,
          },
        ],
      }),
    ).toEqual([
      svc({
        name: 'web',
        cwd: '/app',
        pid: 9,
      }),
    ])
  })

  it('drops nameless / junk entries instead of rendering broken rows', () => {
    expect(
      parsePublishList({
        services: [
          { name: 'ok', port: 1, status: 'stopped' },
          { name: '  ' },
          { name: '' },
          null,
          42,
          { cmd: 'nope' },
        ],
      }).map((s) => s.name),
    ).toEqual(['ok'])
  })

  it('non-object / missing services yields an empty list', () => {
    expect(parsePublishList(null)).toEqual([])
    expect(parsePublishList(undefined)).toEqual([])
    expect(parsePublishList({})).toEqual([])
    expect(parsePublishList({ services: 'nope' })).toEqual([])
  })

  it('accepts last_exit_code as a snake_case alias', () => {
    expect(
      parsePublishList({
        services: [{ name: 'web', last_exit_code: 1, status: 'exited' }],
      })[0]?.lastExitCode,
    ).toBe(1)
  })
})

describe('byoWorkspaceTargets (union leftover nested URLs)', () => {
  const attributed: Record<string, SubdomainTargetInfo> = {
    web: { target: 'localhost:3000', projectId: 'proj-1' },
    staging: { target: 'localhost:4000', projectId: 'proj-1' },
  }

  it('keeps attributed nested URLs whose label does not match a service name', () => {
    expect(byoWorkspaceTargets(attributed, [svc({ name: 'web' })])).toEqual({
      staging: { target: 'localhost:4000', projectId: 'proj-1' },
    })
  })

  it('hides a nested URL that the daemon already hosts as a service', () => {
    expect(byoWorkspaceTargets(attributed, [svc({ name: 'web' }), svc({ name: 'staging' })])).toEqual(
      {},
    )
  })

  it('passes every attributed URL through when there are no services (old remote / BYO-only)', () => {
    expect(byoWorkspaceTargets(attributed, [])).toEqual(attributed)
  })

  it('a service on workspace A never swallows a BYO label on a different name', () => {
    expect(Object.keys(byoWorkspaceTargets(attributed, [svc({ name: 'worker' })])).sort()).toEqual([
      'staging',
      'web',
    ])
  })
})

describe('isLocalOnly / servicePublicUrl', () => {
  it('local expose has no public link even if url is present', () => {
    const local = svc({ name: 'worker', expose: 'local', url: 'https://should-not-show.k2.dev' })
    expect(isLocalOnly(local)).toBe(true)
    expect(servicePublicUrl(local)).toBeNull()
  })

  it('missing url is local-only (no public link)', () => {
    const noUrl = svc({ name: 'web', expose: 'tunnel', url: null })
    expect(isLocalOnly(noUrl)).toBe(true)
    expect(servicePublicUrl(noUrl)).toBeNull()
  })

  it('tunnel + url surfaces the public URL', () => {
    const tun = svc({ name: 'web' })
    expect(isLocalOnly(tun)).toBe(false)
    expect(servicePublicUrl(tun)).toBe('https://web.rosson.k2.dev')
  })
})

describe('serviceListenLabel', () => {
  it('prefers localhost:port when the daemon gave a port', () => {
    expect(serviceListenLabel(svc({ name: 'web', port: 3000, target: '127.0.0.1:3000' }))).toBe(
      'localhost:3000',
    )
  })

  it('falls back to target when port is absent', () => {
    expect(serviceListenLabel(svc({ name: 'web', port: null, target: '127.0.0.1:8080' }))).toBe(
      '127.0.0.1:8080',
    )
  })
})

describe('isServiceStoppable', () => {
  it('Stop while running or starting; Start otherwise (exited / stopped / unhealthy)', () => {
    expect(isServiceStoppable(svc({ name: 'web', status: 'running' }))).toBe(true)
    expect(isServiceStoppable(svc({ name: 'web', status: 'starting' }))).toBe(true)
    expect(isServiceStoppable(svc({ name: 'web', status: 'exited' }))).toBe(false)
    expect(isServiceStoppable(svc({ name: 'web', status: 'stopped' }))).toBe(false)
    expect(isServiceStoppable(svc({ name: 'web', status: 'unhealthy' }))).toBe(false)
  })
})

describe('sortedServices', () => {
  it('sorts by name so the drawer never reshuffles across refreshes', () => {
    expect(
      sortedServices([svc({ name: 'web' }), svc({ name: 'api' }), svc({ name: 'worker' })]).map(
        (s) => s.name,
      ),
    ).toEqual(['api', 'web', 'worker'])
  })
})

describe('unattributedHint', () => {
  it('is null when every nested URL is claimed', () => {
    expect(unattributedHint(0)).toBeNull()
  })

  it('names the claim verb so the unused counter is a one-line hint', () => {
    expect(unattributedHint(1)).toBe(
      '1 nested URL is not claimed to a workspace — k2 publish subdomain claim <label>',
    )
    expect(unattributedHint(3)).toBe(
      '3 nested URLs are not claimed to a workspace — k2 publish subdomain claim <label>',
    )
  })
})

describe('PUBLISH_RUN_EXAMPLE (empty-state copy)', () => {
  it('is the canonical run example, not bare k2 publish', () => {
    expect(PUBLISH_RUN_EXAMPLE).toBe('k2 publish run <name> --cmd "…" --port <n>')
    expect(PUBLISH_RUN_EXAMPLE).toContain('k2 publish run')
    expect(PUBLISH_RUN_EXAMPLE).not.toBe('k2 publish')
  })
})

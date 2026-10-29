// 0.46.7 Workbench: sidecars in a Garden's reach (`ws/name::local`), right
// after their workspace's row, and a sidecar row's address is its Thread.
import { describe, expect, it } from 'vitest'
import { zenSidecarRowsFrom, zenSidecarThread } from './zen-sidecar-reach'
import { withSidecars } from './zen-custom-scope'

describe('sidecars in the Garden reach', () => {
  it('reads the daemon’s rows and leaves out anything that isn’t a local sidecar', () => {
    const rows = zenSidecarRowsFrom({
      ok: true,
      agents: [
        { address: 'K2/Fix::local', thread: 'k2/fix', workspace: 'k2', workspaceLabel: 'K2', workspaceId: 'p1', name: 'fix' },
        { address: 'k2::local', thread: 'k2', workspace: 'k2' },
        { address: 'k2/far::other.k2.dev', thread: 'k2/far', workspace: 'k2' },
        { address: 'k2/fix::local', thread: 'k2/fix', workspace: 'k2' },
        { thread: 'k2/nope', workspace: 'k2' },
        'junk',
      ],
    })
    expect(rows).toEqual([{ address: 'k2/fix::local', workspaceId: 'p1', label: 'fix · K2', parent: 'k2::local', thread: 'k2/fix' }])
    expect(() => zenSidecarRowsFrom({ ok: true })).toThrow(/not a list/)
  })

  it('puts each sidecar right after its workspace, and drops one whose workspace is out of reach', () => {
    const base = [
      { address: 'k2::local', workspaceId: 'p1', label: 'K2' },
      { address: 'sew::local', workspaceId: 'p2', label: 'Sew' },
    ]
    const out = withSidecars(base, [
      { address: 'sew/a::local', workspaceId: null, label: 'a · Sew', parent: 'sew::local', thread: 'sew/a' },
      { address: 'k2/fix::local', workspaceId: 'p1', label: 'fix · K2', parent: 'k2::local', thread: 'k2/fix' },
      { address: 'gone/x::local', workspaceId: null, label: 'x', parent: 'gone::local', thread: 'gone/x' },
    ])
    expect(out.map((r) => r.address)).toEqual(['k2::local', 'k2/fix::local', 'sew::local', 'sew/a::local'])
    expect(out[3].workspaceId).toBe('p2')
    expect(withSidecars(base, [])).toEqual(base)
  })

  it('a sidecar row’s Thread is ws/name; a workspace row has none', () => {
    expect(zenSidecarThread('k2/fix::local')).toBe('k2/fix')
    expect(zenSidecarThread('K2/Fix')).toBe('k2/fix')
    expect(zenSidecarThread('k2::local')).toBeNull()
    expect(zenSidecarThread('k2/a/b::local')).toBeNull()
    expect(zenSidecarThread('/x::local')).toBeNull()
  })
})

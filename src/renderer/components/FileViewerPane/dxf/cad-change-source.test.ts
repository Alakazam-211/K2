// The daemon's change source: while the server has no change route (404,
// 405, 501) or answers anything else that isn't a change, there's no
// highlight; Undo's refusal comes back in the server's own words.

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const cli = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: cli.get, daemonCliPost: cli.post }))

import { CHANGE_ROUTE, daemonCadChangeSource, REVERT_ROUTE } from './cad-change-source'
import type { ServerScope } from '@/kessel/server-scope'

const scope = {} as ServerScope
const source = daemonCadChangeSource(scope)
const MOVE = { schema: 'cad.change/1', format: 'dxf', from: 41, to: 42, pairing: 'kept', versions: [], entities: [], tables: [], blocks: [] }

beforeEach(() => {
  cli.get.mockReset()
  cli.post.mockReset()
})

describe('daemonCadChangeSource', () => {
  it('asks the change route for one version, and reads cad.change/1', async () => {
    cli.get.mockResolvedValue(MOVE)
    expect(await source.change('/w/p.dxf', 41, 42)).toMatchObject({ from: 41, to: 42 })
    expect(cli.get).toHaveBeenCalledWith(scope, CHANGE_ROUTE, { path: '/w/p.dxf', from: 41, to: 42 })
  })

  it('no route yet, a failure, or another shape: null, so the viewer draws as before', async () => {
    for (const status of [404, 405, 501, 500]) {
      cli.get.mockRejectedValueOnce(new DaemonHttpError('route not found', status, null))
      expect(await source.change('/w/p.dxf', 41, 42)).toBeNull()
    }
    cli.get.mockResolvedValueOnce({ schema: 'something/else' })
    expect(await source.change('/w/p.dxf', 41, 42)).toBeNull()
  })

  it('Undo posts the version; a refusal keeps its words; no route says so', async () => {
    cli.post.mockResolvedValueOnce({ ok: true })
    expect(await source.revert('/w/p.dxf', 42)).toEqual({ ok: true })
    expect(cli.post).toHaveBeenCalledWith(scope, REVERT_ROUTE, { path: '/w/p.dxf', version: 42 })
    cli.post.mockRejectedValueOnce(new DaemonHttpError('Sam moved this line since. Undo Sam’s move first.', 409, 'revert_refused'))
    expect(await source.revert('/w/p.dxf', 42)).toEqual({ ok: false, words: 'Sam moved this line since. Undo Sam’s move first.' })
    cli.post.mockRejectedValueOnce(new DaemonHttpError('route not found', 404, null))
    expect(await source.revert('/w/p.dxf', 42)).toEqual({ ok: false, words: 'Undo isn’t on this server yet.' })
  })
})

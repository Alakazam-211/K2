// The daemon's change source: while the server has no change route (404,
// 405, 501) or answers anything else that isn't a change, there's no
// highlight; Undo posts the change's version and the version on screen, and
// its refusal comes back in plain words.

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const cli = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: cli.get, daemonCliPost: cli.post }))

import { CHANGE_ROUTE, daemonCadChangeSource, REVERT_ROUTE, revertBody, UNDO_NOT_LATEST_WORDS } from './cad-change-source'
import { SELF_PID } from './cad-change-marks'
import { changeNote } from './cad-change-words'
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

  const DIGEST = 'sha256:5f1c0e9a7b2d4c6e8f0a1b3c5d7e9f1a2b4c6d8e0f1a3b5c7d9e1f2a4b6c8d0e'

  it('Undo posts the change’s version (the head) and the head’s digest', async () => {
    expect(revertBody('/w/p.dxf', 42, DIGEST)).toEqual({ path: '/w/p.dxf', change_version: 42, expect_digest: DIGEST })
    cli.post.mockResolvedValueOnce({ ok: true, path: '/w/p.dxf', reverted: 42, to: 41, version: 43, digest: 'sha256:abc', change: null })
    expect(await source.revert('/w/p.dxf', 42, DIGEST)).toEqual({ ok: true, change: null })
    expect(cli.post).toHaveBeenCalledWith(scope, REVERT_ROUTE, { path: '/w/p.dxf', change_version: 42, expect_digest: DIGEST })
  })

  it('a landed undo answers its change at the top level: yours, the undo’s verb', async () => {
    const change = { ...MOVE, from: 42, to: 43, entities: [{ address: '@h:41', op: 'changed', kind: 'LINE', layer: 'A-WALL', owner: 'model', fields: ['geom'], bbox_before: [5, 2, 15, 2], bbox_after: [0, 2, 10, 2], bbox_q: 'exact', version: 43, by: 'p_me', identity: null }], verb: 'undo' }
    cli.post.mockResolvedValueOnce({ ok: true, path: '/w/p.dxf', reverted: 42, to: 41, version: 43, digest: 'sha256:abc', change })
    const r = await source.revert('/w/p.dxf', 42, DIGEST)
    expect(r).toMatchObject({ ok: true, change: { from: 42, to: 43, versions: [{ version: 43, by: { pid: SELF_PID, name: 'You' }, verb: 'undo' }] } })
    expect(r.ok && r.change && changeNote(r.change, { index: { handles: {}, layers: {}, layerState: {} } }).text).toBe('You undid a line on A-WALL')
  })

  it('each refusal in plain words; some read the drawing again, some open it view only', async () => {
    const refuse = (status: number, code: string, hint = 'the server’s own words') => {
      const error = { code, hint, exit: 2 }
      cli.post.mockRejectedValueOnce(new DaemonHttpError(hint, status, code, error, { ok: false, error }))
      return source.revert('/w/p.dxf', 42, DIGEST)
    }
    expect(await refuse(409, 'cad_undo_not_latest')).toEqual({ ok: false, words: UNDO_NOT_LATEST_WORDS })
    expect(await refuse(409, 'cad_expect_stale')).toEqual({ ok: false, words: 'The drawing changed; reloaded.', reload: true })
    expect(await refuse(409, 'no_earlier_version')).toEqual({ ok: false, words: 'There’s nothing earlier to go back to.' })
    const hint = 'K2 can’t save drawing edits yet; this file opens read-only.'
    expect(await refuse(409, 'cad_writer_disabled', hint)).toEqual({ ok: false, words: hint, off: hint })
    expect(await refuse(403, 'edit_forbidden')).toEqual({ ok: false, words: 'You can’t change this drawing.' })
    expect(await refuse(400, 'bad_request')).toEqual({ ok: false, words: 'Couldn’t undo this. Please report it with the file.' })
    // Only the full codes: a short form is just the server's message.
    expect(await refuse(409, 'not_latest', 'raw words')).toEqual({ ok: false, words: 'raw words' })
    expect(await refuse(409, 'stale', 'raw words')).toEqual({ ok: false, words: 'raw words' })
    expect(await refuse(403, 'core_off', 'Drawings are off on this server.')).toEqual({ ok: false, words: 'Drawings are off on this server.', off: 'Drawings are off on this server.' })
    cli.post.mockRejectedValueOnce(new DaemonHttpError('route not found', 404, null))
    expect(await source.revert('/w/p.dxf', 42, DIGEST)).toEqual({ ok: false, words: 'Undo isn’t on this server yet.' })
  })
})

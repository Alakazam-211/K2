// The edit verbs' argv, and the flow every edit takes: a dry run against
// the version shown, then the real call with the dry run's digest
// (`--expect`), both as data. The daemon source posts to the doc tool route
// and reads the kit's result object from the reply's `result`; a missing
// route or a thrown answer comes back as a refusal, never an exception: a
// session refusal by its id (or its code until the id comes), a writer
// that's off in the server's own words, remembered per server.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const cli = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: cli.get, daemonCliPost: cli.post }))
type StructureEvent = { workspace?: string | null; rel: string; change: string }
const bus = vi.hoisted(() => ({ fns: new Set<(e: StructureEvent) => void>() }))
vi.mock('@/stores/session-events', () => ({
  onDocStructureChanged: (_s: unknown, fn: (e: StructureEvent) => void) => {
    bus.fns.add(fn)
    return () => void bus.fns.delete(fn)
  },
}))

import type { ServerScope } from '@/kessel/server-scope'
import { attribArgv, fmtCoord, moveArgv, NO_DIGEST, runEdit, withKit } from './edit-flow'
import {
  __resetWriterOffForTests,
  CORES_ROUTE,
  daemonEditSource,
  fixtureEditSource,
  NO_ROUTE,
  parseHolds,
  readOnlyOf,
  sameFile,
  TOOL_ROUTE,
  WHO_ROUTE,
  WRITER_OFF,
} from './edit-source'
import { parseToolResult } from './tool-result'

const FIX = resolve(__dirname, '../../__fixtures__/cad-edit')
const fx = (n: string): unknown => JSON.parse(readFileSync(`${FIX}/${n}.json`, 'utf8'))
const DIGEST = (fx('move-dry') as { changed: { bytes: { after_digest: string } } }).changed.bytes.after_digest

beforeEach(() => {
  cli.get.mockReset()
  cli.post.mockReset()
})

describe('argv', () => {
  it('entity move: every address, then --by dx,dy and the drawing’s unit word', () => {
    expect(moveArgv(['@h:41', '@h:A00'], 5, -2.5, 'mm')).toEqual(['entity', 'move', '@h:41', '@h:A00', '--by', '5,-2.5', '--unit', 'mm'])
    expect(moveArgv(['@h:41'], 0.1 + 0.2, 1e-7, 'drawing')).toEqual(['entity', 'move', '@h:41', '--by', '0.3,0.0000001', '--unit', 'drawing'])
  })

  it('numbers are plain decimals: no exponent, no minus zero', () => {
    expect(fmtCoord(1e21)).toBe('1000000000000000000000')
    expect(fmtCoord(-0)).toBe('0')
    expect(fmtCoord(-0.00000000001)).toBe('0')
    expect(fmtCoord(12.5)).toBe('12.5')
  })

  it('attrib set: the reference, the tag and the value, with --nth only when given', () => {
    expect(attribArgv('@h:61', 'NO', 'D-02')).toEqual(['attrib', 'set', '@h:61', '--tag', 'NO', '--value', 'D-02'])
    expect(attribArgv('@h:61', 'NO', 'D 02', 2)).toEqual(['attrib', 'set', '@h:61', '--tag', 'NO', '--value', 'D 02', '--nth', '2'])
  })

  it('the kit’s flags: --base and --json on every call, then --dry-run or --expect', () => {
    expect(withKit(['entity', 'move'], { base: 7, dryRun: true })).toEqual(['entity', 'move', '--base', '7', '--json', '--dry-run'])
    expect(withKit(['entity', 'move'], { base: 7, expect: DIGEST })).toEqual(['entity', 'move', '--base', '7', '--json', '--expect', DIGEST])
  })
})

describe('runEdit: a dry run, then the commit with --expect', () => {
  const argv = moveArgv(['@h:41'], 5, 0, 'mm')

  it('lands what the dry run checked', async () => {
    const src = fixtureEditSource((_a, n) => (n === 0 ? fx('move-dry') : fx('move-done')))
    const out = await runEdit(src, '/w/a.dxf', argv, 7)
    expect(out.kind).toBe('done')
    expect(src.calls).toEqual([
      [...argv, '--base', '7', '--json', '--dry-run'],
      [...argv, '--base', '7', '--json', '--expect', DIGEST],
    ])
    expect(out.kind === 'done' && out.result.after).toBe(8)
  })

  it('a dry run’s refusal stops there: nothing is committed', async () => {
    const src = fixtureEditSource(() => fx('refused-locked'))
    const out = await runEdit(src, '/w/a.dxf', argv, 7)
    expect(out).toMatchObject({ kind: 'refused', result: { error: { id: 'DXF-E045' } } })
    expect(src.calls).toHaveLength(1)
  })

  it('the commit’s refusal comes back too', async () => {
    const src = fixtureEditSource((_a, n) => (n === 0 ? fx('move-dry') : fx('refused-overlap')))
    const out = await runEdit(src, '/w/a.dxf', argv, 7)
    expect(out).toMatchObject({ kind: 'refused', result: { error: { id: 'MPC-E723' } } })
  })

  it('a value that’s already so: no commit', async () => {
    const src = fixtureEditSource(() => fx('attrib-no-change'))
    expect(await runEdit(src, '/w/a.dxf', attribArgv('@h:61', 'NO', 'D-01'), 7)).toEqual({ kind: 'unchanged' })
    expect(src.calls).toHaveLength(1)
  })

  it('a dry run without a digest is never committed blind', async () => {
    const dry = fx('move-dry') as { changed: { bytes: { after_digest: string | null } } }
    dry.changed.bytes.after_digest = null
    const src = fixtureEditSource(() => dry)
    expect(await runEdit(src, '/w/a.dxf', argv, 7)).toMatchObject({ kind: 'refused', result: { error: { id: NO_DIGEST } } })
    expect(src.calls).toHaveLength(1)
  })

  it('while the server is still reading the file, waits as told and tries once more', async () => {
    const answers = [fx('refused-pending'), fx('move-dry'), fx('move-done')]
    const src = fixtureEditSource((_a, n) => answers[n])
    const wait = vi.fn(async () => undefined)
    expect((await runEdit(src, '/w/a.dxf', argv, 7, wait)).kind).toBe('done')
    expect(wait).toHaveBeenCalledWith(250)
    expect(src.calls[0]).toEqual(src.calls[1])
  })
})

describe('daemonEditSource', () => {
  const scope = {} as ServerScope
  const src = daemonEditSource(scope)

  it('posts the argv to the doc tool route as data, and reads the result from the reply', async () => {
    cli.post.mockResolvedValue({ doc: {}, exit: 0, stdout: '', result: fx('move-done') })
    const r = await src.run('/w/a.dxf', ['entity', 'move', '@h:41'])
    expect(cli.post).toHaveBeenCalledWith(scope, TOOL_ROUTE, { path: '/w/a.dxf', argv: ['entity', 'move', '@h:41'], json: true })
    expect(r).toMatchObject({ ok: true, base: 7, after: 8, verb: 'entity move' })
  })

  it('a verb’s own refusal is the result under `result`; a reply’s own error with an id is that refusal', async () => {
    cli.post.mockResolvedValueOnce({ exit: 2, stdout: '', result: fx('refused-locked') })
    expect((await src.run('/w/a.dxf', ['x'])).error?.id).toBe('DXF-E045')
    cli.post.mockResolvedValueOnce({ exit: 2, stdout: '', error: { id: 'CAD-E010', message: '@h:99 names no entity', details: {} } })
    expect((await src.run('/w/a.dxf', ['x'])).error?.id).toBe('CAD-E010')
  })

  it('the whole reply is never read as the result', async () => {
    cli.post.mockResolvedValueOnce(fx('move-done'))
    expect((await src.run('/w/a.dxf', ['x'])).error?.id).toBe(NO_ROUTE)
  })

  it('no route, or a reply without a result: a refusal of its own', async () => {
    cli.post.mockRejectedValueOnce(new DaemonHttpError('route not found', 404, null))
    expect((await src.run('/w/a.dxf', ['x'])).error?.id).toBe(NO_ROUTE)
    cli.post.mockResolvedValueOnce({ doc: {}, exit: 0, stdout: 'moved' })
    expect((await src.run('/w/a.dxf', ['x'])).error?.id).toBe(NO_ROUTE)
  })

  it('a session refusal (409) by its id, or by its code until the id comes', async () => {
    const e409 = (error: Record<string, unknown>) => new DaemonHttpError(String(error.hint ?? 'refused'), 409, (error.code as string) ?? null, error, { ok: false, error })
    cli.post.mockRejectedValueOnce(e409({ code: 'leased', id: 'MPC-E701', exit: 5, hint: 'Sam has the lease', holder: 'Sam' }))
    expect((await src.run('/w/a.dxf', ['x'])).error).toMatchObject({ id: 'MPC-E701', details: { holder: 'Sam' } })
    cli.post.mockRejectedValueOnce(e409({ code: 'leased', exit: 5, hint: 'Sam has the lease', holder: 'Sam', retryAfterMs: 2000 }))
    expect((await src.run('/w/a.dxf', ['x'])).error?.id).toBe('MPC-E701')
    cli.post.mockRejectedValueOnce(e409({ code: 'claimed', exit: 5, by: 'Atlas' }))
    expect((await src.run('/w/a.dxf', ['x'])).error).toMatchObject({ id: 'MPC-E301', details: { by: 'Atlas' } })
    cli.post.mockRejectedValueOnce(new DaemonHttpError('changed since', 409, 'MPC-E110'))
    expect((await src.run('/w/a.dxf', ['x'])).error?.id).toBe('MPC-E110')
  })

  it('a writer that’s off: the server’s words, remembered for that server for the session', async () => {
    __resetWriterOffForTests()
    const hint = 'K2 can’t save drawing edits yet; this file opens read-only.'
    const a = daemonEditSource({ hostKey: 'host-a' } as ServerScope)
    const again = daemonEditSource({ hostKey: 'host-a' } as ServerScope)
    const b = daemonEditSource({ hostKey: 'host-b' } as ServerScope)
    expect(a.writerOff()).toBeNull()
    const error = { code: 'cad_writer_disabled', exit: 3, hint, path: '/w/a.dxf' }
    cli.post.mockRejectedValueOnce(new DaemonHttpError(hint, 409, 'cad_writer_disabled', error, { ok: false, error }))
    expect((await a.run('/w/a.dxf', ['x'])).error).toMatchObject({ id: WRITER_OFF, message: hint })
    expect(again.writerOff()).toBe(hint)
    expect(b.writerOff()).toBeNull()
    __resetWriterOffForTests()
  })

  it('read only up front: docs/cores’ words for the file, read tolerantly', async () => {
    cli.get.mockResolvedValueOnce({ file: { core: 'cad', on: true, liveView: false, read_only_reason: 'Drawings open read-only on this server.' } })
    expect(await src.readOnly('/w/a.dxf')).toBe('Drawings open read-only on this server.')
    expect(cli.get).toHaveBeenLastCalledWith(scope, CORES_ROUTE, { path: '/w/a.dxf' })
    cli.get.mockResolvedValueOnce({ file: { core: 'cad', on: true, liveView: false } })
    expect(await src.readOnly('/w/a.dxf')).toBeNull()
    cli.get.mockRejectedValueOnce(new Error('offline'))
    expect(await src.readOnly('/w/a.dxf')).toBeNull()
    expect(readOnlyOf({ file: { core: 'cad', on: true, liveView: false, read_only_reason: ' x ' } })).toBe('x')
    expect(readOnlyOf({ file: { core: 'cad', on: true, liveView: false } })).toBeNull()
    expect(readOnlyOf({ read_only_reason: 'not where it lives' })).toBeNull()
  })

  it('holds are read again when a claim on this file is taken or ends', () => {
    const fn = vi.fn()
    const off = src.onHoldsChanged('/ws/drawings/a.dxf', fn)
    for (const f of bus.fns) {
      f({ workspace: '/ws', rel: 'drawings/a.dxf', change: 'claim_taken' })
      f({ workspace: '/ws/', rel: 'drawings/a.dxf', change: 'claim_ended' })
      f({ workspace: '/ws', rel: 'drawings/a.dxf', change: 'annotation' })
      f({ workspace: '/ws', rel: 'drawings/b.dxf', change: 'claim_taken' })
      // Another workspace with the same path in it: not this file.
      f({ workspace: '/other', rel: 'drawings/a.dxf', change: 'claim_taken' })
      // No workspace on the event: the path's end.
      f({ workspace: null, rel: 'drawings/a.dxf', change: 'claim_taken' })
      f({ rel: 'drawings/a.dxf', change: 'claim_ended' })
    }
    expect(fn).toHaveBeenCalledTimes(4)
    off()
    expect(bus.fns.size).toBe(0)
    expect(sameFile('/ws/a.dxf', 'a.dxf')).toBe(true)
    expect(sameFile('/ws/ba.dxf', 'a.dxf')).toBe(false)
    expect(sameFile('/ws/a.dxf', 'a.dxf', '/ws')).toBe(true)
    expect(sameFile('/other/ws/a.dxf', 'a.dxf', '/ws')).toBe(false)
  })

  it('holds: agents’ claims by their node keys, each with what it’s for, from who’s on the file', async () => {
    cli.get.mockResolvedValue({
      roster: [
        { pid: 'p_atlas', name: 'Atlas', kind: 'agent', claims: [{ nodes: ['@h:41', '@h:A00'], hold: true, note: 'moving the doors' }, { nodes: null }, { nodes: ['@h:61'], note: ' ' }] },
        { pid: 'p_sam', name: 'Sam', kind: 'person', claims: [{ nodes: ['@h:42'] }] },
        { pid: 'p_kai', name: 'Kai', kind: 'agent', claims: [] },
      ],
    })
    expect(await src.holds('/w/a.dxf')).toEqual([
      { pid: 'p_atlas', name: 'Atlas', note: 'moving the doors', keys: ['@h:41', '@h:A00'] },
      { pid: 'p_atlas', name: 'Atlas', note: null, keys: ['@h:61'] },
    ])
    expect(cli.get).toHaveBeenCalledWith(scope, WHO_ROUTE, { path: '/w/a.dxf' })
    cli.get.mockRejectedValueOnce(new Error('offline'))
    expect(await src.holds('/w/a.dxf')).toEqual([])
    expect(parseHolds(null)).toEqual([])
  })
})

describe('parseToolResult', () => {
  it('reads a dry run: no version made, the digest to store', () => {
    expect(parseToolResult(fx('move-dry'))).toMatchObject({ ok: true, dryRun: true, after: null, afterDigest: DIGEST })
  })
  it('anything else is not a result', () => {
    expect(parseToolResult({ schema: 'cad.change/1' })).toBeNull()
    expect(parseToolResult('moved')).toBeNull()
  })
})

// The real scene source on frames K2's own daemon sent. Captured on compute
// (2026-11-16) from k2-daemon on K2's wire-packs branch at 8a5878b13b: a
// harness daemon with the blend core on, the pack's living-room .blend
// opened over WS /cli/docs/live, a watch, a second socket selecting an
// object, and a write through the CLI route. Kept byte for byte in
// tests/fixtures/blend-live/room_capture.json; replayed here frame by frame
// through the kit client, as the docs socket delivered them.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import type { ServerFrame, Transport } from '@k2/mpc-client'
import { parseLiveDocCore, stateFor } from '../../live-doc-core'
import { KeyedWatchSceneSource, type KeyedWelcome } from './keyed-source'
import { BLEND_WRITER_OFF, refusalWhy } from './wording'
import type { SceneSnapshot } from './sample-source'

type J = Record<string, unknown>
const CAP = JSON.parse(readFileSync(join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'blend-live', 'room_capture.json'), 'utf8')) as {
  cores: { file: { core: string; on: boolean; liveView: boolean } }
  welcome: J[]
  watch: J[]
  write: { status: number; body: { ok: boolean; error: { code: string; hint?: string } } }
}

function socket(): Transport & { push: (f: unknown) => void; sent: J[] } {
  const subs = new Set<(f: ServerFrame) => void>()
  const sent: J[] = []
  return {
    send: (f: unknown) => void sent.push(f as J),
    onFrame: (cb: (f: ServerFrame) => void) => {
      subs.add(cb)
      return () => subs.delete(cb)
    },
    push: (f: unknown) => {
      for (const cb of [...subs]) cb(f as ServerFrame)
    },
    sent,
  } as Transport & { push: (f: unknown) => void; sent: J[] }
}

function replay() {
  const t = socket()
  const src = new KeyedWatchSceneSource(t, { post: async () => undefined, fileName: 'room', now: () => 1793111400000 })
  const [welcome, ...rest] = CAP.welcome
  src.attach(welcome as unknown as KeyedWelcome)
  for (const f of rest) t.push(f)
  for (const f of CAP.watch) t.push(f)
  return { t, src, snap: () => src.snapshot() as SceneSnapshot }
}

describe('the real daemon’s frames (k2-daemon, a living-room .blend)', () => {
  it('docs/cores: blend, on, liveView false; the tab opens the real scene source all the same', () => {
    expect(CAP.cores.file).toEqual({ core: 'blend', on: true, liveView: false })
    expect(stateFor(parseLiveDocCore(CAP.cores)).status).toBe('scene')
  })

  it('the welcome: the scene family, no claims, rights a word, read only with the daemon’s own words', () => {
    const w = CAP.welcome[0]
    expect(w).toMatchObject({ type: 'welcome', core: 'blend', family: 'scene', rights: 'read', features: ['keyed_watch', 'view_address', 'presence'] })
    const { snap } = replay()
    expect(snap().readOnly).toBe(w.read_only_reason)
    expect(snap().scene.holds).toBe(false)
  })

  it('connects and gets the snapshot: every page in, the scene drawn, nothing still loading', () => {
    const { src, snap, t } = replay()
    expect(src.kind).toBe('live')
    expect(t.sent.map((f) => f.type)).toEqual(['watch', 'report'])
    const S = src.state()
    expect(S.loading).toBeNull()
    expect(S.D.lists.colls).toEqual(['@coll:1', '@coll:2', '@coll:3', '@coll:4', '@coll:0'])
    expect(S.D.lists.objs).toHaveLength(26)
    expect(S.D.currentScene).toBe('@coll:1')
    expect(S.D.props['@coll:3'].name).toBe('Furniture')
    expect(S.D.props['@coll:4'].parent).toBe('@coll:3')
    expect(snap().scene.declined).toBeFalsy()
    expect(snap().people.map((p) => [p.id, p.name, p.viewAddress])).toEqual([['person:owner', 'You', '@scene:main']])
  })

  it('a second socket’s select reaches presence (your other window: the kit has it; the scene lists you once)', () => {
    const { src, snap } = replay()
    const other = src.presence!.peers().find((p) => p.sameUser)
    expect(other?.selection).toEqual({ root: 'objs', keys: ['@obj:1'] })
    expect(snap().people).toHaveLength(1)
  })

  it('a write is refused: 409 blend_writer_disabled, and the words are the hint, never the code', () => {
    expect(CAP.write.status).toBe(409)
    expect(CAP.write.body.error.code).toBe(BLEND_WRITER_OFF)
    const why = refusalWhy(CAP.write.body.error.code, CAP.write.body.error.hint)
    expect(why).toBe(CAP.write.body.error.hint)
    expect(why).not.toContain(BLEND_WRITER_OFF)
  })
})

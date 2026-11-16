// The scene's default view is `@scene:main`: the kit refuses a bare
// `@scene` (K2 Render, scene listing corrections 2026-11-16). No view
// address the sample hands the shell (people's views, the say address) is
// anything the kit's rules can't parse, and no scene code or sample data
// spells the bare form; only the mock's naming adapter still reads it.

import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { SCENARIOS } from './scenarios-more'
import { sceneSnapshot } from './sample-source'
import { simulate } from './turn'
import { SCENE_VIEW, parseView } from './view-address'
import { SCENE_SHELL } from '@/components/LiveView/scene/scene-shell'

const BARE = /(['"`])@scene\1/

function files(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? files(join(dir, e.name)) : /\.(ts|tsx|json)$/.test(e.name) && !/\.test\./.test(e.name) ? [join(dir, e.name)] : []))
}

describe('the scene’s default view address', () => {
  it('is @scene:main, and a bare @scene isn’t an address', () => {
    expect(SCENE_VIEW).toBe('@scene:main')
    expect(parseView('@scene')).toBeNull()
  })

  it('every view the sample hands the shell parses, in every step of every scenario', () => {
    for (const sc of SCENARIOS)
      sc.steps.forEach((_, i) => {
        const snap = sceneSnapshot(simulate(sc, i), sc, i)
        for (const p of snap.people) if (p.viewAddress != null) expect(parseView(p.viewAddress), `${sc.id}@${i} ${p.name}: ${p.viewAddress}`).not.toBeNull()
        for (const p of snap.people) {
          const say = SCENE_SHELL.sayAddress?.(snap, p.id)
          if (say) expect(parseView(say.address), `${sc.id}@${i} say to ${p.name}: ${say.address}`).not.toBeNull()
        }
      })
  })

  it('no scene code or sample spells the bare form (the mock-naming adapter aside)', () => {
    const scene = join(__dirname)
    const shell = join(__dirname, '..', '..', '..', 'components', 'LiveView', 'scene')
    const bad = [...files(scene), ...files(shell)].filter((f) => !f.endsWith('sample-views.ts') && BARE.test(readFileSync(f, 'utf8')))
    expect(bad.map((f) => f.slice(f.indexOf('src/')))).toEqual([])
  })
})

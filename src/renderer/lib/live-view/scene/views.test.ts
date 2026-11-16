// The view picker's rows and the keys on the picture, on K2 Render's view
// addresses (`@scene:main`, `@cam:<id>`, `@obj:<id>`, `@turntable:<n>`):
// Home Scene, 0 the file's main camera, . frames the selection, Esc goes
// back; a key that can't act here does nothing (and isn't swallowed). And
// the sample's own (the mock's) view names read as K2 Render's.

import { describe, expect, it } from 'vitest'
import { DOCS } from './fold'
import { GOLD } from './samples'
import { fromMockView } from './sample-views'
import { KEY_HINT, cameraView, goView, mainCamera, objectOfView, objectView, sameView, stillView, viewKey, viewName, viewRows, type ViewNav } from './views'
import type { Doc } from './types'
import { SCENARIOS } from './scenarios-more'
import { simulate } from './turn'
import { sceneSnapshot } from './sample-source'

const D = DOCS[GOLD.state.upto]
const NAV0: ViewNav = { view: null, back: [] }

describe('views of the doc', () => {
  it('names an object’s and a camera’s view by their live keys', () => {
    expect(objectView(D, '@obj:10')).toBe('@obj:10')
    expect(objectView(D, '@obj:19')).toBe('@obj:19')
    expect(cameraView(D, '@obj:1')).toBe('@cam:1')
    expect(cameraView(D, '@obj:26')).toBe('@cam:26')
    expect(objectOfView(D, '@obj:19')).toBe('@obj:19')
    expect(objectOfView(D, '@cam:26')).toBe('@obj:26')
    // a camera view of an object that isn't a camera, and an object not in the doc
    expect(objectOfView(D, '@cam:10')).toBeNull()
    expect(objectOfView(D, '@obj:999')).toBeNull()
  })

  it('two objects with one name keep their own views (keys, not names)', () => {
    const twin: Doc = { ...D, props: { ...D.props, '@obj:11': { ...D.props['@obj:11'], name: 'Table' } } }
    expect(objectView(twin, '@obj:10')).toBe('@obj:10')
    expect(objectView(twin, '@obj:11')).toBe('@obj:11')
    expect(objectOfView(twin, '@obj:11')).toBe('@obj:11')
  })

  it('reads the sample’s (the mock’s) view names as K2 Render’s', () => {
    expect(fromMockView(D, '@scene')).toBe('@scene:main')
    expect(fromMockView(D, '@cam:999')).toBe('@scene:main')
    expect(fromMockView(D, '@cam:26')).toBe('@cam:26')
    expect(fromMockView(D, '@obj:10')).toBe('@obj:10')
    expect(fromMockView(D, '@scene/turntable', 3)).toBe('@turntable:3')
    expect(fromMockView(D, '@obj:*')).toBe('@obj:*')
    expect(fromMockView(D, '@obj:10')).toBe('@obj:10')
    expect(fromMockView(D, null)).toBeNull()
  })

  it('the sample’s people carry K2 Render’s view addresses (Follow maps them straight onto a view)', () => {
    const sc = SCENARIOS.find((s) => s.id === 'who')!
    const snap = sceneSnapshot(simulate(sc, 3), sc, 3)
    expect(snap.people.find((p) => p.id === 'agent:brook')?.viewAddress).toBe('@cam:26')
    expect(snap.people.find((p) => p.id === 'agent:atlas')?.viewAddress).toBe('@obj:10')
  })
})

describe('the view picker', () => {
  it('lists Scene, the file’s cameras as Camera 1 and 2, Framed object and Turntable, in the mock’s words', () => {
    const rows = viewRows(D, { framed: null, agent: null })
    expect(rows.map((r) => [r.v, r.name, r.sub, r.key, r.disabled])).toEqual([
      ['@scene:main', 'Scene', 'The whole room', 'Home', false],
      ['@cam:1', 'Camera 1', 'The file’s main camera', '0', false],
      ['@cam:26', 'Camera 2', 'A camera in the file', '', false],
      ['@obj:*', 'Framed object', 'Pick an object first', '.', true],
      ['@turntable:0', 'Turntable', '8 stills around the scene', '', false],
    ])
  })

  it('frames what is selected, and offers the agent’s view under a divider', () => {
    const rows = viewRows(D, { framed: '@obj:10', agent: { name: 'Atlas', view: '@obj:10' } })
    expect(rows[3]).toMatchObject({ v: '@obj:10', sub: 'Table', disabled: false })
    expect(rows[5]).toMatchObject({ v: '@agent', name: 'Atlas’s view', sub: 'Follows Atlas: now Table, framed', sep: true })
  })

  it('names views plainly', () => {
    expect(viewName(D, '@scene:main')).toBe('Scene')
    expect(viewName(D, '@cam:26')).toBe('Camera 2')
    expect(viewName(D, '@obj:10')).toBe('Table, framed')
    expect(viewName(D, '@obj:19')).toBe('Floor lamp, framed')
    expect(viewName(D, '@turntable:4')).toBe('Turntable')
    expect(sameView('@obj:10', '@obj:15')).toBe(true)
    expect(sameView('@turntable:3', '@turntable:0')).toBe(true)
    expect(sameView('@cam:1', '@cam:26')).toBe(false)
  })

  it('a turntable’s thumbnail is another frame of it', () => {
    expect(stillView('@turntable:0', 1)).toBe('@turntable:1')
    expect(stillView('@turntable:0', 12)).toBe('@turntable:7')
    expect(stillView('@cam:1', 3)).toBe('@cam:1')
  })

  it('shows the key hint “Home Scene · 0 Camera · . Frame · Esc Back”', () => {
    expect(KEY_HINT.map((k) => `${k.key} ${k.label}`).join(' · ')).toBe('Home Scene · 0 Camera · . Frame · Esc Back')
  })
})

describe('keys on the picture', () => {
  const o = { cur: '@scene:main', camera: mainCamera(D), framed: '@obj:10' }

  it('0 goes to the file’s main camera, . frames the selection, Home goes to Scene', () => {
    expect(mainCamera(D)).toBe('@cam:1')
    const a = viewKey(NAV0, '0', o)!
    expect(a).toEqual({ view: '@cam:1', back: ['@scene:main'] })
    const b = viewKey(a, '.', { ...o, cur: '@cam:1' })!
    expect(b).toEqual({ view: '@obj:10', back: ['@scene:main', '@cam:1'] })
    const c = viewKey(b, 'Home', { ...o, cur: '@obj:10' })!
    expect(c).toEqual({ view: '@scene:main', back: ['@scene:main', '@cam:1', '@obj:10'] })
  })

  it('Esc goes back one view at a time, then does nothing', () => {
    let nav: ViewNav | null = { view: '@obj:10', back: ['@scene:main', '@cam:1'] }
    nav = viewKey(nav, 'Escape', o)
    expect(nav).toEqual({ view: '@cam:1', back: ['@scene:main'] })
    nav = viewKey(nav!, 'Escape', o)
    expect(nav).toEqual({ view: '@scene:main', back: [] })
    expect(viewKey(nav!, 'Escape', o)).toBeNull()
  })

  it('does nothing for . with nothing selected, 0 with no camera, or another key', () => {
    expect(viewKey(NAV0, '.', { ...o, framed: null })).toBeNull()
    expect(viewKey(NAV0, '0', { ...o, camera: null })).toBeNull()
    expect(viewKey(NAV0, 'a', o)).toBeNull()
  })

  it('Home on Scene keeps the back stack as it is', () => {
    expect(goView({ view: '@scene:main', back: ['@cam:1'] }, '@scene:main', '@scene:main')).toEqual({ view: '@scene:main', back: ['@cam:1'] })
  })
})

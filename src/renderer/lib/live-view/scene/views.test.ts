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
  it('names an object’s and a camera’s view by their names, encoded', () => {
    expect(objectView(D, '@obj:10')).toBe('@obj:Table')
    expect(objectView(D, '@obj:19')).toBe('@obj:Floor%20lamp')
    expect(cameraView(D, '@obj:1')).toBe('@cam:Camera')
    expect(cameraView(D, '@obj:26')).toBe('@cam:Camera.001')
    expect(objectOfView(D, '@obj:Floor%20lamp')).toBe('@obj:19')
    expect(objectOfView(D, '@cam:Camera.001')).toBe('@obj:26')
    expect(objectOfView(D, '@cam:Table')).toBeNull()
    expect(objectOfView(D, '@obj:Nothing')).toBeNull()
  })

  it('falls back to @obj-n:<index> only when two objects share a name', () => {
    const twin: Doc = { ...D, props: { ...D.props, '@obj:11': { ...D.props['@obj:11'], name: 'Table' } } }
    const i = twin.lists.objs.indexOf('@obj:10')
    expect(objectView(twin, '@obj:10')).toBe(`@obj-n:${i}`)
    expect(objectOfView(twin, `@obj-n:${i}`)).toBe('@obj:10')
    expect(objectView(twin, '@obj:15')).toBe('@obj:Sofa')
  })

  it('reads the sample’s (the mock’s) view names as K2 Render’s', () => {
    expect(fromMockView(D, '@scene')).toBe('@scene:main')
    expect(fromMockView(D, '@cam:26')).toBe('@cam:Camera.001')
    expect(fromMockView(D, '@obj:10')).toBe('@obj:Table')
    expect(fromMockView(D, '@scene/turntable', 3)).toBe('@turntable:3')
    expect(fromMockView(D, '@obj:*')).toBe('@obj:*')
    expect(fromMockView(D, '@obj:Table')).toBe('@obj:Table')
    expect(fromMockView(D, null)).toBeNull()
  })

  it('the sample’s people carry K2 Render’s view addresses (Follow maps them straight onto a view)', () => {
    const sc = SCENARIOS.find((s) => s.id === 'who')!
    const snap = sceneSnapshot(simulate(sc, 3), sc, 3)
    expect(snap.people.find((p) => p.id === 'agent:brook')?.viewAddress).toBe('@cam:Camera.001')
    expect(snap.people.find((p) => p.id === 'agent:atlas')?.viewAddress).toBe('@obj:Table')
  })
})

describe('the view picker', () => {
  it('lists Scene, the file’s cameras as Camera 1 and 2, Framed object and Turntable, in the mock’s words', () => {
    const rows = viewRows(D, { framed: null, agent: null })
    expect(rows.map((r) => [r.v, r.name, r.sub, r.key, r.disabled])).toEqual([
      ['@scene:main', 'Scene', 'The whole room', 'Home', false],
      ['@cam:Camera', 'Camera 1', 'The file’s main camera', '0', false],
      ['@cam:Camera.001', 'Camera 2', 'A camera in the file', '', false],
      ['@obj:*', 'Framed object', 'Pick an object first', '.', true],
      ['@turntable:0', 'Turntable', '8 stills around the scene', '', false],
    ])
  })

  it('frames what is selected, and offers the agent’s view under a divider', () => {
    const rows = viewRows(D, { framed: '@obj:Table', agent: { name: 'Atlas', view: '@obj:Table' } })
    expect(rows[3]).toMatchObject({ v: '@obj:Table', sub: 'Table', disabled: false })
    expect(rows[5]).toMatchObject({ v: '@agent', name: 'Atlas’s view', sub: 'Follows Atlas: now Table, framed', sep: true })
  })

  it('names views plainly', () => {
    expect(viewName(D, '@scene:main')).toBe('Scene')
    expect(viewName(D, '@cam:Camera.001')).toBe('Camera 2')
    expect(viewName(D, '@obj:Table')).toBe('Table, framed')
    expect(viewName(D, '@obj:Floor%20lamp')).toBe('Floor lamp, framed')
    expect(viewName(D, '@turntable:4')).toBe('Turntable')
    expect(sameView('@obj:Table', '@obj:Sofa')).toBe(true)
    expect(sameView('@turntable:3', '@turntable:0')).toBe(true)
    expect(sameView('@cam:Camera', '@cam:Camera.001')).toBe(false)
  })

  it('a turntable’s thumbnail is another frame of it', () => {
    expect(stillView('@turntable:0', 1)).toBe('@turntable:1')
    expect(stillView('@turntable:0', 12)).toBe('@turntable:7')
    expect(stillView('@cam:Camera', 3)).toBe('@cam:Camera')
  })

  it('shows the key hint “Home Scene · 0 Camera · . Frame · Esc Back”', () => {
    expect(KEY_HINT.map((k) => `${k.key} ${k.label}`).join(' · ')).toBe('Home Scene · 0 Camera · . Frame · Esc Back')
  })
})

describe('keys on the picture', () => {
  const o = { cur: '@scene:main', camera: mainCamera(D), framed: '@obj:Table' }

  it('0 goes to the file’s main camera, . frames the selection, Home goes to Scene', () => {
    expect(mainCamera(D)).toBe('@cam:Camera')
    const a = viewKey(NAV0, '0', o)!
    expect(a).toEqual({ view: '@cam:Camera', back: ['@scene:main'] })
    const b = viewKey(a, '.', { ...o, cur: '@cam:Camera' })!
    expect(b).toEqual({ view: '@obj:Table', back: ['@scene:main', '@cam:Camera'] })
    const c = viewKey(b, 'Home', { ...o, cur: '@obj:Table' })!
    expect(c).toEqual({ view: '@scene:main', back: ['@scene:main', '@cam:Camera', '@obj:Table'] })
  })

  it('Esc goes back one view at a time, then does nothing', () => {
    let nav: ViewNav | null = { view: '@obj:Table', back: ['@scene:main', '@cam:Camera'] }
    nav = viewKey(nav, 'Escape', o)
    expect(nav).toEqual({ view: '@cam:Camera', back: ['@scene:main'] })
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
    expect(goView({ view: '@scene:main', back: ['@cam:Camera'] }, '@scene:main', '@scene:main')).toEqual({ view: '@scene:main', back: ['@cam:Camera'] })
  })
})

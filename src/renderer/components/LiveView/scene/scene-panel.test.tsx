// @vitest-environment jsdom
// The scene tab's side panel, live from the sample: Activity, People and
// holds, Outliner, Properties, in that order, with the mock's heads and
// counts, the "Find an object" field, the outliner's rows (nesting, the eye,
// what can't change, who holds it, who is on it), picking a row, the keys,
// a search with no match, and Properties' transform table.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { SampleSceneSource } from '@/lib/live-view/scene/sample-source'
import { LiveShellView } from '../shell/LiveShellView'
import { SCENE_SHELL } from './scene-shell'

let src: SampleSceneSource | null = null
afterEach(() => {
  cleanup()
  src?.close()
})

function mount(scenario: string, step = 0, width = 1440): SampleSceneSource {
  src = new SampleSceneSource({ autoplay: false, scenario })
  src.player.goTo(step)
  render(<LiveShellView source={src} config={SCENE_SHELL} width={width} />)
  return src
}
const section = (id: string): HTMLElement => screen.getByTestId(`lv-scene-section-${id}`)
const head = (id: string): HTMLElement => screen.getByTestId(`lv-scene-head-${id}`)
const row = (name: string): HTMLElement => within(screen.getByRole('tree', { name: /Collections and objects/ })).getByText(name, { selector: '.lv-orow-name' }).closest('[role="treeitem"]') as HTMLElement

describe('the scene panel', () => {
  it('runs Activity, People and holds, Outliner, Properties, with their counts', () => {
    mount('who')
    const ids = Array.from(screen.getByTestId('lv-panel').querySelectorAll('[data-testid^="lv-scene-section-"]')).map((s) => s.getAttribute('data-testid'))
    expect(ids).toEqual(['lv-scene-section-activity', 'lv-scene-section-people', 'lv-scene-section-outliner', 'lv-scene-section-props'])
    expect(within(section('people')).getByTestId('lv-scene-count').textContent).toBe('4')
    expect(within(section('outliner')).getByTestId('lv-scene-count').textContent).toBe('26')
    expect(head('activity').getAttribute('aria-expanded')).toBe('true')
  })

  it('at 800 the panel opens with People and Properties closed', () => {
    mount('who', 0, 800)
    fireEvent.click(screen.getByRole('button', { name: 'Show the side panel' }))
    expect(head('people').getAttribute('aria-expanded')).toBe('false')
    expect(head('props').getAttribute('aria-expanded')).toBe('false')
    expect(head('outliner').getAttribute('aria-expanded')).toBe('true')
    fireEvent.click(head('people'))
    expect(head('people').getAttribute('aria-expanded')).toBe('true')
  })

  it('Activity leads with what the agent is doing now', () => {
    mount('who')
    const first = within(section('activity')).getAllByTestId('lv-scene-activity')[0]
    expect(first.textContent).toContain('Atlas is arranging the dining set')
    expect(first.getAttribute('data-kind')).toBe('now')
  })

  it('People: name and view on one line, what they do below; clicking someone follows them', () => {
    mount('turn', 1)
    const atlas = within(section('people')).getByRole('button', { name: /^Atlas/ })
    expect(atlas.textContent).toContain('Table, framed')
    expect(atlas.textContent).toContain('arranging the dining set')
    fireEvent.click(atlas)
    expect(within(section('people')).getByRole('button', { name: /^Atlas/ }).textContent).toContain('You’re following')
    expect(screen.getByRole('button', { name: 'Following Atlas · Stop' })).toBeTruthy()
  })

  it('the outliner nests collections and objects, with the eye, what can’t change, holds and people', () => {
    mount('who')
    expect(screen.getByRole('searchbox', { name: 'Find an object by name' }).getAttribute('placeholder')).toBe('Find an object')
    expect(row('Room').getAttribute('aria-level')).toBe('2')
    expect(row('Floor').getAttribute('aria-level')).toBe('3')
    expect(within(row('Wall.Left')).getByTitle('Hidden in the viewport')).toBeTruthy()
    expect(within(row('Wall.Back')).getByTitle('Shown')).toBeTruthy()
    expect(within(row('Ceiling fan')).getByTitle('Turns by quaternion: its rotation can’t change live')).toBeTruthy()
    expect(within(row('Floor')).getByTitle(/^Sam/)).toBeTruthy()
  })

  it('a hold shows on its rows, and Properties says who holds it', () => {
    mount('refuse')
    expect(within(row('Table')).getByTitle('Held by Atlas: arranging the dining set')).toBeTruthy()
    const props = section('props')
    expect(within(props).getByTestId('lv-scene-prop-name').textContent).toBe('Table')
    expect(props.textContent).toContain('Held by Atlas: arranging the dining set')
  })

  it('picking a row selects it and shows its numbers: X Y Z, the units in the labels, 3 decimals', () => {
    mount('refuse')
    fireEvent.click(row('Sofa'))
    expect(row('Sofa').getAttribute('aria-selected')).toBe('true')
    const props = section('props')
    expect(within(props).getByTestId('lv-scene-prop-name').textContent).toBe('Sofa')
    const table = within(props).getByRole('table', { name: 'Transform' })
    expect(within(table).getAllByRole('columnheader').map((c) => c.textContent)).toEqual(['', 'X', 'Y', 'Z'])
    expect(within(table).getAllByRole('rowheader').map((c) => c.textContent)).toEqual(['Location (m)', 'Rotation (°)', 'Scale'])
    for (const c of within(table).getAllByRole('cell')) expect(c.textContent).toMatch(/^-?\d+\.\d{3}$/)
  })

  it('a value that just changed is marked, with what it was Before', () => {
    mount('turn', 4)
    const props = section('props')
    expect(within(props).getByTestId('lv-scene-prop-name').textContent).toBe('Table')
    expect(within(props).getByText('Before')).toBeTruthy()
    expect(props.querySelectorAll('[data-changed]').length).toBeGreaterThan(0)
  })

  it('the keys: Down moves through the rows, Left closes a collection, Right opens it', () => {
    mount('O3')
    const tree = screen.getByRole('tree', { name: /Collections and objects/ })
    act(() => tree.focus())
    expect(tree.getAttribute('aria-activedescendant')).toBe(row('Scene').id)
    fireEvent.keyDown(tree, { key: 'ArrowDown' })
    expect(tree.getAttribute('aria-activedescendant')).toBe(row('Room').id)
    fireEvent.keyDown(tree, { key: 'ArrowLeft' })
    expect(row('Room').getAttribute('aria-expanded')).toBe('false')
    fireEvent.keyDown(tree, { key: 'ArrowRight' })
    expect(row('Room').getAttribute('aria-expanded')).toBe('true')
    fireEvent.keyDown(tree, { key: 'ArrowDown' })
    expect(row('Floor').getAttribute('aria-selected')).toBe('true')
  })

  it('Find an object: no match says so and offers Clear the search, which brings the list back', () => {
    mount('who')
    fireEvent.change(screen.getByRole('searchbox', { name: 'Find an object by name' }), { target: { value: 'Piano' } })
    expect(screen.getByRole('status').textContent).toContain('No object is named “Piano”.')
    fireEvent.click(screen.getByRole('button', { name: 'Clear the search' }))
    expect(row('Floor')).toBeTruthy()
  })

  it('a large scene draws only the rows on screen', () => {
    mount('large', 1)
    const tree = screen.getByRole('tree', { name: /Collections and objects/ })
    expect(Number(tree.getAttribute('data-rows'))).toBe(121)
    expect(tree.querySelectorAll('[role="treeitem"]').length).toBeLessThan(60)
  })

  it('the banner picks any of the 24 scenarios, grouped', () => {
    const s = mount('who')
    expect(s.player.scenarioList()).toHaveLength(24)
    expect(within(screen.getByTestId('lv-sample-banner')).getByTestId('lv-sample-scenarios').textContent).toContain('Who is on what')
  })
})

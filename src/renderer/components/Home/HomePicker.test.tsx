// @vitest-environment jsdom
//
// The Home picker's "New Home…" field starts on the next free "Group N",
// selected, so typing replaces it and Enter accepts it. Existing names are
// never changed.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import HomePicker from './HomePicker'
import { useHomesStore, selectedHome } from '@/stores/homes'

/** Leave exactly these Homes, in order (the store refuses to drop the last). */
function setHomeNames(names: string[]): void {
  const st = useHomesStore.getState()
  const before = st.homes.map((h) => h.id)
  for (const n of names) if (st.createHome(n) === null) throw new Error(`createHome refused ${n}`)
  for (const id of before) if (!useHomesStore.getState().deleteHome(id)) throw new Error(`deleteHome refused ${id}`)
  expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(names)
}

function openCreate(): HTMLInputElement {
  render(<HomePicker />)
  fireEvent.click(screen.getByTitle('Pick a Home'))
  fireEvent.click(screen.getByRole('menuitem', { name: 'New Home…' }))
  return screen.getByRole('textbox') as HTMLInputElement
}

beforeEach(() => {
  localStorage.clear()
  sessionStorage.clear()
})

afterEach(() => cleanup())

describe('New Home… default name', () => {
  it('pre-fills the next free Group N, selected, with it as the hint', () => {
    setHomeNames(['Home', 'Group 1', 'group 3'])
    const input = openCreate()
    expect(input.value).toBe('Group 2')
    expect(input.placeholder).toBe('Group 2')
    expect(document.activeElement).toBe(input)
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 'Group 2'.length])
  })

  it('Enter accepts the suggested name; the existing names stay', () => {
    setHomeNames(['Home', 'Group 1'])
    const input = openCreate()
    act(() => {
      fireEvent.submit(input.closest('form') as HTMLFormElement)
    })
    expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(['Home', 'Group 1', 'Group 2'])
    expect(selectedHome(useHomesStore.getState()).name).toBe('Group 2')
  })

  it('a typed name replaces the suggestion', () => {
    setHomeNames(['Group 1'])
    const input = openCreate()
    fireEvent.change(input, { target: { value: 'Sales' } })
    act(() => {
      fireEvent.submit(input.closest('form') as HTMLFormElement)
    })
    expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(['Group 1', 'Sales'])
  })
})

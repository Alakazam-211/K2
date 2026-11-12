// @vitest-environment jsdom
//
// The board's name in its tab: renaming the file from inside the board (Rosson 2026-11-11).
import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { BOARD_MENU_SEARCH_AT, BoardFileName, boardNameOf, boardNameProblem, boardRenameRefusal, type BoardMenuRow, type BoardMenuSource } from './BoardFileName'

const PATH = '/w/Plan.k5board'
afterEach(cleanup)

const row = (name: string, dir = '/w'): BoardMenuRow => ({ path: `${dir}/${name}.k5board`, name, rel: `${name}.k5board` })
/** A list the test answers by hand: `answer(rows)` or `refuse()`. */
function source(): BoardMenuSource & { answer(rows: BoardMenuRow[]): void; refuse(): void; subs: number } {
  let push: ((rows: BoardMenuRow[]) => void) | null = null
  let fail: ((err: unknown) => void) | null = null
  const s = {
    subs: 0,
    subscribe(p: (rows: BoardMenuRow[]) => void, f: (err: unknown) => void) {
      s.subs++
      push = p
      fail = f
      return () => undefined
    },
    answer: (rows: BoardMenuRow[]) => act(() => push?.(rows)),
    refuse: () => act(() => fail?.(new Error('no'))),
  }
  return s as unknown as BoardMenuSource & { answer(rows: BoardMenuRow[]): void; refuse(): void; subs: number }
}

const draw = (
  rename = vi.fn(async () => undefined),
  boards: BoardMenuSource | undefined = source(),
  pick = vi.fn(),
): { rename: typeof rename; pick: typeof pick } => {
  render(<BoardFileName filePath={PATH} rename={rename} boards={boards} pick={pick} />)
  return { rename, pick }
}
const startEdit = (): void => key(nameBtn(), 'F2')
const nameBtn = (): HTMLElement => screen.getByTestId('board-file-name')
const field = (): HTMLInputElement => screen.getByTestId('board-file-name-input') as HTMLInputElement
const type = (v: string): void => void fireEvent.change(field(), { target: { value: v } })
const key = (el: HTMLElement, k: string): void => void fireEvent.keyDown(el, { key: k })
const settle = (): Promise<void> => act(async () => undefined)

describe('the name', () => {
  it('is the file name without .k5board, a menu button that says how to rename', () => {
    draw()
    expect(nameBtn().textContent).toBe('Plan')
    expect(nameBtn().getAttribute('aria-label')).toBe('Board name: Plan. Press Enter for your boards, F2 to rename')
    expect(nameBtn().getAttribute('aria-haspopup')).toBe('menu')
    expect(boardNameOf('/a/b/Road map.K5BOARD')).toBe('Road map')
  })

  it('F2 starts an edit, with the name in the field', () => {
    draw()
    startEdit()
    expect(field().value).toBe('Plan')
    expect(field().getAttribute('aria-label')).toBe('Board name')
  })

  it('Rename… in the menu starts the edit too', () => {
    draw()
    fireEvent.click(nameBtn())
    fireEvent.click(screen.getByRole('menuitem', { name: /Rename…/ }))
    expect(field().value).toBe('Plan')
    expect(screen.queryByRole('menu')).toBeNull()
  })

  it.each([
    ['a click', (): void => void fireEvent.click(nameBtn())],
    ['Enter', (): void => key(nameBtn(), 'Enter')],
  ])('without a list (no switcher), %s starts the edit', (_how, go) => {
    draw(undefined, null as unknown as undefined) // null: no switcher (undefined would take the default list)
    go()
    expect(field().value).toBe('Plan')
  })
})

describe('saving', () => {
  it('Enter renames the file, the extension added back', async () => {
    const { rename } = draw()
    startEdit()
    type('  Road   map ')
    key(field(), 'Enter')
    await settle()
    expect(rename).toHaveBeenCalledWith('Road map.k5board')
    expect(screen.queryByTestId('board-file-name-input')).toBeNull()
  })

  it('blur with a change saves; blur without one cancels', async () => {
    const { rename } = draw()
    startEdit()
    fireEvent.blur(field())
    await settle()
    expect(rename).not.toHaveBeenCalled()
    expect(screen.queryByTestId('board-file-name-input')).toBeNull()

    startEdit()
    type('Later')
    fireEvent.blur(field())
    await settle()
    expect(rename).toHaveBeenCalledWith('Later.k5board')
  })

  it('Esc cancels, and nothing is sent', async () => {
    const { rename } = draw()
    startEdit()
    type('Other')
    key(field(), 'Escape')
    await settle()
    expect(rename).not.toHaveBeenCalled()
    expect(nameBtn().textContent).toBe('Plan')
  })
})

describe('refusals', () => {
  it.each([
    ['an empty name', '   ', 'Give the board a name.'],
    ['a slash', 'a/b', 'A board name can’t have a slash.'],
    ['a backslash', 'a\\b', 'A board name can’t have a slash.'],
  ])('%s is refused here: a message by the field, still editing, nothing sent', async (_what, v, said) => {
    const { rename } = draw()
    startEdit()
    type(v)
    key(field(), 'Enter')
    await settle()
    expect(rename).not.toHaveBeenCalled()
    expect(screen.getByTestId('board-file-name-problem').textContent).toBe(said)
    expect(field().getAttribute('aria-invalid')).toBe('true')
  })

  it('a taken name says so plainly, and the field stays', async () => {
    const rename = vi.fn(async () => {
      throw new Error('Already exists: /w/Taken.k5board')
    })
    draw(rename)
    startEdit()
    type('Taken')
    key(field(), 'Enter')
    await settle()
    expect(screen.getByTestId('board-file-name-problem').textContent).toBe('A board with that name is already here.')
    expect(field().value).toBe('Taken')
  })

  it('any other failure is general, with the daemon’s reason quietly after it', async () => {
    const rename = vi.fn(async () => {
      throw new Error('Permission denied')
    })
    draw(rename)
    startEdit()
    type('Mine')
    key(field(), 'Enter')
    await settle()
    expect(screen.getByTestId('board-file-name-problem').textContent).toBe('Couldn’t rename the board.Permission denied')
    expect(field()).toBeTruthy()
  })

  it('the pure checks', () => {
    expect(boardNameProblem('ok')).toBeNull()
    expect(boardRenameRefusal(new Error('Already exists: x')).reason).toBeNull()
  })
})

describe('the switcher (this workspace’s boards)', () => {
  const menu = (): HTMLElement => screen.getByRole('menu', { name: 'Your boards' })
  const rows = (): string[] => screen.queryAllByRole('menuitemradio').map((el) => el.textContent ?? '')

  it.each([
    ['a click', (): void => void fireEvent.click(nameBtn())],
    ['ArrowDown', (): void => key(nameBtn(), 'ArrowDown')],
  ])('%s opens the menu: loading, then the boards with this one marked', async (_how, go) => {
    const s = source()
    draw(undefined, s)
    go()
    expect(menu().textContent).toContain('Loading boards…')
    s.answer([row('Ideas'), row('Plan'), row('Road map')])
    expect(rows()).toEqual(['Ideas', '✓Plan', 'Road map'])
    const here = screen.getByRole('menuitemradio', { name: /Plan/ })
    expect(here.getAttribute('aria-checked')).toBe('true')
    expect(document.activeElement).toBe(here)
  })

  it('picking one opens it in this tab; picking this one does nothing', () => {
    const s = source()
    const { pick } = draw(undefined, s)
    fireEvent.click(nameBtn())
    s.answer([row('Ideas'), row('Plan')])
    fireEvent.click(screen.getByRole('menuitemradio', { name: /Plan/ }))
    expect(pick).not.toHaveBeenCalled()
    fireEvent.click(nameBtn())
    s.answer([row('Ideas'), row('Plan')])
    fireEvent.click(screen.getByRole('menuitemradio', { name: /Ideas/ }))
    expect(pick).toHaveBeenCalledWith('/w/Ideas.k5board')
    expect(screen.queryByRole('menu')).toBeNull()
  })

  it('keys: arrows move, type-ahead jumps, Enter picks, Esc closes back to the name', () => {
    const s = source()
    const { pick } = draw(undefined, s)
    fireEvent.click(nameBtn())
    s.answer([row('Alpha'), row('Plan'), row('Zed')])
    key(document.activeElement as HTMLElement, 'ArrowDown')
    expect(document.activeElement?.textContent).toBe('Zed')
    key(document.activeElement as HTMLElement, 'ArrowDown')
    expect(document.activeElement?.textContent).toContain('Rename…')
    key(document.activeElement as HTMLElement, 'a')
    expect(document.activeElement?.textContent).toBe('Alpha')
    fireEvent.click(document.activeElement as HTMLElement)
    expect(pick).toHaveBeenCalledWith('/w/Alpha.k5board')
    fireEvent.click(nameBtn())
    key(menu(), 'Escape')
    expect(screen.queryByRole('menu')).toBeNull()
    expect(document.activeElement).toBe(nameBtn())
  })

  it('no other board: says so, and Rename… is still there', () => {
    const s = source()
    draw(undefined, s)
    fireEvent.click(nameBtn())
    s.answer([row('Plan')])
    expect(menu().textContent).toContain('No other boards here.')
    expect(screen.getByRole('menuitem', { name: /Rename…/ })).toBeTruthy()
  })

  it('a list that can’t be read: a plain line and Try again, which reads again', () => {
    const s = source()
    draw(undefined, s)
    fireEvent.click(nameBtn())
    s.refuse()
    expect(screen.getByTestId('board-file-menu-error').textContent).toContain('K2 couldn’t list your boards.')
    fireEvent.click(screen.getByRole('menuitem', { name: 'Try again' }))
    expect(s.subs).toBe(2)
    s.answer([row('Plan'), row('Other')])
    expect(rows()).toEqual(['✓Plan', 'Other'])
  })

  it(`more than ${BOARD_MENU_SEARCH_AT} boards: a search field that filters by name`, () => {
    const s = source()
    draw(undefined, s)
    fireEvent.click(nameBtn())
    s.answer([row('Plan'), row('Other')])
    expect(screen.queryByTestId('board-file-menu-search')).toBeNull()
    const many = [row('Plan'), ...Array.from({ length: BOARD_MENU_SEARCH_AT }, (_, i) => row(`Board ${i + 1}`)), row('Roadmap')]
    s.answer(many)
    const search = screen.getByTestId('board-file-menu-search') as HTMLInputElement
    fireEvent.change(search, { target: { value: 'road' } })
    expect(rows()).toEqual(['Roadmap'])
    fireEvent.change(search, { target: { value: 'nothing like it' } })
    expect(menu().textContent).toContain('No boards match.')
  })

  it('two boards with one name show where each is', () => {
    const s = source()
    draw(undefined, s)
    fireEvent.click(nameBtn())
    s.answer([row('Plan'), { path: '/w/old/Plan.k5board', name: 'Plan', rel: 'old/Plan.k5board' }])
    expect(rows()).toEqual(['✓PlanPlan.k5board', 'Planold/Plan.k5board'])
  })
})

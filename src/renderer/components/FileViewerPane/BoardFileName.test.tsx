// @vitest-environment jsdom
//
// The board's name in its tab: renaming the file from inside the board (Rosson 2026-11-11).
import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { BoardFileName, boardNameOf, boardNameProblem, boardRenameRefusal } from './BoardFileName'

const PATH = '/w/Plan.k5board'
afterEach(cleanup)

const draw = (rename = vi.fn(async () => undefined)): { rename: typeof rename } => {
  render(<BoardFileName filePath={PATH} rename={rename} />)
  return { rename }
}
const nameBtn = (): HTMLElement => screen.getByTestId('board-file-name')
const field = (): HTMLInputElement => screen.getByTestId('board-file-name-input') as HTMLInputElement
const type = (v: string): void => void fireEvent.change(field(), { target: { value: v } })
const key = (el: HTMLElement, k: string): void => void fireEvent.keyDown(el, { key: k })
const settle = (): Promise<void> => act(async () => undefined)

describe('the name', () => {
  it('is the file name without .k5board, with a name that says how to rename it', () => {
    draw()
    expect(nameBtn().textContent).toBe('Plan')
    expect(nameBtn().getAttribute('aria-label')).toBe('Board name: Plan, press Enter to rename')
    expect(boardNameOf('/a/b/Road map.K5BOARD')).toBe('Road map')
  })

  it.each([
    ['a click', (): void => void fireEvent.click(nameBtn())],
    ['F2', (): void => key(nameBtn(), 'F2')],
    ['Enter', (): void => key(nameBtn(), 'Enter')],
  ])('%s starts an edit, with the name in the field', (_how, go) => {
    draw()
    go()
    expect(field().value).toBe('Plan')
    expect(field().getAttribute('aria-label')).toBe('Board name')
  })
})

describe('saving', () => {
  it('Enter renames the file, the extension added back', async () => {
    const { rename } = draw()
    fireEvent.click(nameBtn())
    type('  Road   map ')
    key(field(), 'Enter')
    await settle()
    expect(rename).toHaveBeenCalledWith('Road map.k5board')
    expect(screen.queryByTestId('board-file-name-input')).toBeNull()
  })

  it('blur with a change saves; blur without one cancels', async () => {
    const { rename } = draw()
    fireEvent.click(nameBtn())
    fireEvent.blur(field())
    await settle()
    expect(rename).not.toHaveBeenCalled()
    expect(screen.queryByTestId('board-file-name-input')).toBeNull()

    fireEvent.click(nameBtn())
    type('Later')
    fireEvent.blur(field())
    await settle()
    expect(rename).toHaveBeenCalledWith('Later.k5board')
  })

  it('Esc cancels, and nothing is sent', async () => {
    const { rename } = draw()
    fireEvent.click(nameBtn())
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
    fireEvent.click(nameBtn())
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
    fireEvent.click(nameBtn())
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
    fireEvent.click(nameBtn())
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

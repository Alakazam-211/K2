// @vitest-environment jsdom
// 0.46.7 Workbench: K2's own sheet for a widget's Land, Retire, Discard or
// Start. K2's words from the daemon's change; the main button is yes,
// Cancel and Esc are no. Agent text (commit subjects, a brief) is text.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { ZenWorkSheet, zenWorkSheetWords } from './ZenWorkSheet'

afterEach(cleanup)

const land = {
  kind: 'land' as const,
  name: 'fix',
  thread: 'k2/fix',
  branch: 'k2/fix',
  base: 'main',
  onto: 'aaaaaaa',
  ahead: 2,
  commits: [
    { sha: '1', short: 'abc1234', subject: 'Keep the cookie <img src=x onerror=alert(1)>' },
    { sha: '2', short: 'def5678', subject: 'Tests' },
  ],
  files: [{ file: 'src/login.ts' }],
  uncommitted: 1,
}

describe('ZenWorkSheet', () => {
  it('says what each change does in K2’s words', () => {
    expect(zenWorkSheetWords(land)).toEqual({ title: 'Land fix’s 2 commit(s) on main?', go: 'Land on main', danger: false })
    expect(zenWorkSheetWords({ kind: 'settle', name: 'fix', discard: true }).go).toBe('Discard')
    expect(zenWorkSheetWords({ kind: 'settle', name: 'fix', discard: false }).go).toBe('Retire')
    expect(zenWorkSheetWords({ kind: 'start', name: 'fix', workspace: 'K2' }).title).toBe('Start fix in K2?')
  })

  it('shows the commits and files as text; Land is yes, Cancel and Esc are no', () => {
    const done = vi.fn()
    render(<ZenWorkSheet change={land} widget="Review" onDone={done} />)
    const commits = screen.getByTestId('zen-work-commits')
    expect(commits.textContent).toContain('Keep the cookie <img src=x onerror=alert(1)>')
    expect(commits.querySelector('img')).toBeNull()
    expect(screen.getByTestId('zen-work-files').textContent).toContain('src/login.ts')
    expect(screen.getByText('1 uncommitted file(s) stay in the agent’s worktree.')).toBeTruthy()
    fireEvent.click(screen.getByTestId('zen-work-go'))
    expect(done).toHaveBeenLastCalledWith(true)
    fireEvent.click(screen.getByTestId('zen-work-cancel'))
    expect(done).toHaveBeenLastCalledWith(false)
    fireEvent.keyDown(screen.getByTestId('zen-work-sheet'), { key: 'Escape' })
    expect(done).toHaveBeenLastCalledWith(false)
    expect(done).toHaveBeenCalledTimes(3)
  })

  it('Start shows the worktree, the branch and the brief', () => {
    render(
      <ZenWorkSheet
        change={{ kind: 'start', name: 'fix', workspace: 'K2', worktree: '.worktrees/k2-fix', branch: 'k2/fix', base: 'main', brief: 'Fix the login.' }}
        widget="Work inbox"
        onDone={() => undefined}
      />,
    )
    expect(screen.getByTestId('zen-work-sheet').getAttribute('data-kind')).toBe('start')
    expect(screen.getByTestId('zen-work-brief').textContent).toBe('Fix the login.')
    expect(screen.getByTestId('zen-work-sheet').textContent).toContain('.worktrees/k2-fix')
  })
})

// @vitest-environment jsdom
import { describe, expect, it, afterEach } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'
import { useState } from 'react'
import { SessionViewTabs } from './SessionViewTabs'
import type { SessionViewTab } from './sessionViewTab'

function Harness({ initial = 'terminal' as SessionViewTab }) {
  const [tab, setTab] = useState<SessionViewTab>(initial)
  return <SessionViewTabs value={tab} onChange={setTab} />
}

describe('SessionViewTabs (C1/C2/C3)', () => {
  afterEach(() => cleanup())

  it('labels Thread and Terminal, default Terminal selected', () => {
    render(<Harness />)
    expect(screen.getByTestId('session-view-tab-thread').textContent).toBe('Thread')
    expect(screen.getByTestId('session-view-tab-terminal').textContent).toBe('Terminal')
    expect(screen.getByTestId('session-view-tab-thread').textContent).not.toBe('Agent')
    expect(screen.getByTestId('session-view-tab-terminal').getAttribute('aria-selected')).toBe(
      'true',
    )
    expect(screen.getByTestId('session-view-tab-thread').getAttribute('aria-selected')).toBe(
      'false',
    )
  })

  it('uses the Feedback underline class on the active tab', () => {
    render(<Harness />)
    const terminal = screen.getByTestId('session-view-tab-terminal')
    expect(terminal.className).toContain('border-b-2')
    expect(terminal.className).toContain('border-[var(--color-accent)]')
  })

  it('switches to Thread on click', () => {
    render(<Harness />)
    fireEvent.click(screen.getByTestId('session-view-tab-thread'))
    expect(screen.getByTestId('session-view-tab-thread').getAttribute('aria-selected')).toBe(
      'true',
    )
  })
})

// @vitest-environment jsdom
//
// Terminal notifications sit at the top right of the visible pane.
// Fail loud: a window-fixed or bottom-anchored stack is a regression.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'
import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore, type AppPage } from '@/stores/page-view'
import { useToastStore } from '@/stores/toast'
import Toast from './Toast'

function stack(): HTMLElement {
  const el = document.querySelector('[data-toast-stack]')
  if (!(el instanceof HTMLElement)) throw new Error('toast stack missing')
  return el
}

function expectTopRightOf(host: HTMLElement): void {
  const el = stack()
  if (el.parentElement !== host) {
    throw new Error('toast stack is not inside the visible pane')
  }
  if (el.style.position === 'fixed' || el.classList.contains('fixed')) {
    throw new Error('toast stack is position:fixed on the window')
  }
  expect(el.style.position).toBe('absolute')
  expect(el.style.top).not.toBe('')
  expect(el.style.right).not.toBe('')
  expect(el.style.bottom).toBe('')
  expect(el.className).not.toContain('bottom-4')
  expect(el.className).not.toMatch(/\bfixed\b/)
  expect(el.className).toContain('flex-col')
  expect(el.className).not.toContain('flex-col-reverse')
  expect(document.querySelectorAll('[data-toast-stack]')).toHaveLength(1)
}

function show(message: string): void {
  act(() => {
    useToastStore.getState().addToast(message, 'info')
  })
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'setInterval', 'clearTimeout', 'clearInterval'] })
  useToastStore.setState({ toasts: [] })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.setState({ page: 'agents', wikiProjectPath: null })
})

afterEach(() => {
  cleanup()
  useToastStore.setState({ toasts: [] })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.setState({ page: 'agents', wikiProjectPath: null })
  vi.useRealTimers()
})

describe('Toast stack placement', () => {
  it('pins one toast to the top right inside the visible terminal pane', () => {
    render(
      <>
        <div data-terminal-container="" data-testid="grid" />
        <Toast />
      </>,
    )
    show('Saved')

    const host = screen.getByTestId('grid')
    expectTopRightOf(host)
    expect(host.contains(stack())).toBe(true)
    expect(screen.getByText('Saved')).toBeTruthy()
  })

  it('prefers the on-screen grid over the session body and a pane wrapper', () => {
    render(
      <>
        <div data-testid="agent-session-terminal">
          <div data-toast-host="pane" data-testid="pane">
            <div data-terminal-container="" data-testid="grid" />
          </div>
        </div>
        <Toast />
      </>,
    )
    show('Grid')

    expectTopRightOf(screen.getByTestId('grid'))
    expect(screen.getByTestId('pane')).not.toBe(stack().parentElement)
  })

  it('anchors to the session body when the grid is display:none', () => {
    render(
      <>
        <div data-testid="agent-session-terminal" data-test-session="">
          <div style={{ display: 'none' }}>
            <div data-terminal-container="" data-testid="grid" />
          </div>
        </div>
        <Toast />
      </>,
    )
    show('Chat')

    const session = document.querySelector('[data-test-session]')
    if (!(session instanceof HTMLElement)) throw new Error('session stand-in missing')
    expectTopRightOf(session)
    expect(screen.getByTestId('grid').contains(stack())).toBe(false)
  })

  it('anchors a file or browser pane under the tab strip when no grid is visible', () => {
    render(
      <>
        <div data-toast-host="pane" data-testid="file-pane">
          <button type="button">file</button>
        </div>
        <Toast />
      </>,
    )
    show('File')

    expectTopRightOf(screen.getByTestId('file-pane'))
  })

  it('keeps a single stack on the focused pane when two grids are visible', () => {
    render(
      <>
        <div data-pane-item-id="a">
          <div data-terminal-container="" data-testid="grid-a" tabIndex={0} />
        </div>
        <div data-pane-item-id="b">
          <div data-terminal-container="" data-testid="grid-b" tabIndex={0} />
        </div>
        <Toast />
      </>,
    )
    show('Split')
    expect(document.querySelectorAll('[data-toast-stack]')).toHaveLength(1)

    act(() => {
      screen.getByTestId('grid-b').focus()
      screen.getByTestId('grid-b').dispatchEvent(new FocusEvent('focusin', { bubbles: true }))
    })

    expectTopRightOf(screen.getByTestId('grid-b'))
    expect(screen.getByTestId('grid-a').contains(stack())).toBe(false)
  })

  it('appends the next toast below the first', () => {
    render(
      <>
        <div data-terminal-container="" data-testid="grid" />
        <Toast />
      </>,
    )
    show('First')
    show('Second')

    const text = stack().textContent ?? ''
    const firstAt = text.indexOf('First')
    const secondAt = text.indexOf('Second')
    if (firstAt < 0 || secondAt < 0 || firstAt > secondAt) {
      throw new Error(`expected First above Second, got ${text}`)
    }
    expectTopRightOf(screen.getByTestId('grid'))
  })

  it('keeps the centered bottom branch while Settings is open', () => {
    useSettingsStore.setState({ settingsOpen: true })
    render(
      <>
        <div data-terminal-container="" data-testid="grid" />
        <Toast />
      </>,
    )
    show('Settings saved')

    const el = stack()
    expect(screen.getByTestId('grid').contains(el)).toBe(false)
    expect(el.classList.contains('fixed')).toBe(true)
    expect(el.className).toContain('bottom-4')
    expect(el.style.left).toBe('50%')
    expect(el.style.transform).toContain('translateX(-50%)')
    expect(el.style.top).toBe('')
    expect(el.style.right).toBe('')
  })

  it('pins Projects, Feedback, and Wiki to that page, not a buried terminal', () => {
    const pages: Array<Extract<AppPage, 'projects' | 'feedback' | 'wiki'>> = [
      'projects',
      'feedback',
      'wiki',
    ]
    for (const page of pages) {
      cleanup()
      useToastStore.setState({ toasts: [] })
      usePageViewStore.setState({ page, wikiProjectPath: null })
      render(
        <>
          <div data-terminal-container="" data-testid="grid" />
          <div data-toast-host={page} data-testid="page-host" />
          <Toast />
        </>,
      )
      show(page)

      expectTopRightOf(screen.getByTestId('page-host'))
      expect(screen.getByTestId('grid').contains(stack())).toBe(false)
    }
  })

  it('does not portal into a terminal while the front page host is display:none', () => {
    usePageViewStore.setState({ page: 'projects' })
    render(
      <>
        <div data-terminal-container="" data-testid="grid" />
        <div data-toast-host="projects" style={{ display: 'none' }} />
        <Toast />
      </>,
    )
    show('Buried')

    expect(document.querySelector('[data-toast-stack]')).toBeNull()
    expect(screen.queryByText('Buried')).toBeNull()
  })
})

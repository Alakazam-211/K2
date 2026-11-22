// @vitest-environment jsdom
// The one-click switch between the macOS browser underlay and the previous
// behaviour. Fail loud: the stored value and the store both follow a click.
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { LS_BROWSER_LAYERING, useBrowserLayeringStore } from '@/stores/browser-layering'
import { BROWSER_LAYERING_LABEL, BrowserLayeringRow } from './BrowserLayeringRow'

beforeEach(() => {
  // The macOS default (jsdom is not a Mac, so the store read legacy).
  useBrowserLayeringStore.setState({ mode: 'underlay', failed: null })
})

afterEach(() => {
  cleanup()
  localStorage.clear()
  useBrowserLayeringStore.setState({ mode: 'underlay', failed: null })
})

describe('BrowserLayeringRow', () => {
  it('is on (underlay) by default on macOS and one click goes back to legacy', () => {
    render(<BrowserLayeringRow mac />)
    const toggle = screen.getByRole('switch', { name: BROWSER_LAYERING_LABEL })
    expect(document.querySelector('[data-settings-id="general.browser-layering"]')).not.toBeNull()
    fireEvent.click(toggle)
    expect(useBrowserLayeringStore.getState().mode).toBe('legacy')
    expect(localStorage.getItem(LS_BROWSER_LAYERING)).toBe('legacy')
    fireEvent.click(toggle)
    expect(useBrowserLayeringStore.getState().mode).toBe('underlay')
    expect(localStorage.getItem(LS_BROWSER_LAYERING)).toBe('underlay')
  })

  it('renders nothing off macOS', () => {
    const { container } = render(<BrowserLayeringRow mac={false} />)
    expect(container.innerHTML).toBe('')
  })

  it('says when the underlay could not start in this window', () => {
    useBrowserLayeringStore.setState({ mode: 'underlay', failed: 'no layer' })
    render(<BrowserLayeringRow mac />)
    expect(screen.getByText(/could not start in this window/)).not.toBeNull()
  })
})

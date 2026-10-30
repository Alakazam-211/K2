// @vitest-environment jsdom
// Settings → General → Start page (Rosson 2026-10-30): the page a window
// opens on, Agents (default) or Home, per client in localStorage. It applies
// at launch and after a sign-in that switches the window, never over a page
// already chosen (a pick or a deep link).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

type PageView = typeof import('./page-view')

/** A fresh window: the page-view module loaded anew (a reload). */
async function launch(): Promise<PageView> {
  vi.resetModules()
  return import('./page-view')
}

beforeEach(() => {
  localStorage.clear()
})
afterEach(() => {
  cleanup()
  localStorage.clear()
})

describe('the start page', () => {
  it('defaults to Agents (today\'s behaviour); anything odd stored reads as Agents', async () => {
    let pv = await launch()
    expect(pv.readStartPage()).toBe('agents')
    expect(pv.usePageViewStore.getState().page).toBe('agents')
    localStorage.setItem(pv.LS_START_PAGE, 'projects')
    pv = await launch()
    expect(pv.usePageViewStore.getState().page).toBe('agents')
  })

  it('Home sticks after a reload', async () => {
    let pv = await launch()
    pv.writeStartPage('home')
    expect(localStorage.getItem('k2.startPage')).toBe('home')
    pv = await launch()
    expect(pv.usePageViewStore.getState().page).toBe('home')
    pv = await launch()
    expect(pv.usePageViewStore.getState().page).toBe('home')
  })

  it('after a sign-in it opens the start page, but a deep link (or any pick) already made wins', async () => {
    let pv = await launch()
    // Signed in with nothing chosen yet: the start page.
    pv.writeStartPage('home')
    pv.applyStartPage()
    expect(pv.usePageViewStore.getState().page).toBe('home')
    // A deep link to Projects, then the sign-in: Projects stays.
    pv = await launch()
    pv.usePageViewStore.getState().setPage('projects')
    pv.applyStartPage()
    expect(pv.usePageViewStore.getState().page).toBe('projects')
    // A wiki deep link counts too.
    pv = await launch()
    pv.usePageViewStore.getState().openWiki('/ws/a')
    pv.applyStartPage()
    expect(pv.usePageViewStore.getState().page).toBe('wiki')
  })

  it('Settings → General: a two-choice control, Agents by default; Home is remembered', async () => {
    await launch()
    const { StartPageRow } = await import('@/components/Settings/sections/StartPageRow')
    render(<StartPageRow />)
    const group = screen.getByRole('radiogroup', { name: 'Start page' })
    const [home, agents] = Array.from(group.querySelectorAll('[role="radio"]'))
    expect([home.textContent, agents.textContent]).toEqual(['Home', 'Agents'])
    expect(agents.getAttribute('aria-checked')).toBe('true')
    expect(home.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(home)
    expect(home.getAttribute('aria-checked')).toBe('true')
    expect(localStorage.getItem('k2.startPage')).toBe('home')
    // It changes the next launch, not the page on screen now.
    const pv = await import('./page-view')
    expect(pv.usePageViewStore.getState().page).toBe('agents')
  })
})

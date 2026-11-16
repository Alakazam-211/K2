// @vitest-environment jsdom
// Providers / Models, side by side like Settings → Projects (the shared
// halves, controls/halves): a fixed header, then Starting agents and Launch
// presets on the left and Agent tools on this computer on the right, each
// half scrolling on its own. Under HALVES_MIN_WIDTH CSS px of body the
// halves stack, the tools below, in one scroll. Reading and keyboard order
// stay left half first.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

import { AgentsSection } from './AgentsSection'
import { HALVES_MIN_WIDTH } from '../controls/halves'
import * as daemonCli from '@/lib/daemon-cli'
import { usePresetsStore, type AgentPreset } from '@/stores/presets'
import { applyAppZoomForTest, resetAppZoomForTest } from '@/test-utils/app-zoom'

// The halves' measured body width drives split vs stacked (jsdom has no
// layout, so the test sets it).
let bodyWidth = 1000
class FakeResizeObserver {
  observe(): void {}
  disconnect(): void {}
  unobserve(): void {}
}
const realRect = Element.prototype.getBoundingClientRect

const preset = (id: string, label: string, command: string): AgentPreset => ({
  id, label, command, icon: '', enabled: 1, sortOrder: 0, isBuiltIn: 1, createdAt: 1, inject_flow: null,
})
const realFetch = usePresetsStore.getState().fetchPresets

beforeEach(() => {
  bodyWidth = 1000
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
    if ((this as HTMLElement).dataset?.testid === 'agents-halves') {
      return { width: bodyWidth, height: 600, top: 0, left: 0, right: bodyWidth, bottom: 600, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
    }
    return realRect.call(this)
  }
  usePresetsStore.setState({
    presets: [preset('b1', 'Claude', 'claude'), preset('b2', 'Codex', 'codex')],
    presetsLoaded: true, presetsError: null, showPresetsBar: true, fetchPresets: vi.fn(async () => {}),
  })
  vi.spyOn(daemonCli, 'daemonCliGet').mockImplementation((async () => new Promise(() => {})) as typeof daemonCli.daemonCliGet)
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  Element.prototype.getBoundingClientRect = realRect
  usePresetsStore.setState({ presets: [], presetsLoaded: false, presetsError: null, fetchPresets: realFetch })
})

function headingsIn(el: HTMLElement): string[] {
  return within(el).queryAllByRole('heading', { level: 3 }).map((h) => h.textContent ?? '')
}

describe('Providers / Models layout', () => {
  it('wide: Starting agents and Launch presets on the left, Agent tools on the right, each scrolling on its own', () => {
    render(<AgentsSection />)
    const halves = screen.getByTestId('agents-halves')
    expect(halves.getAttribute('data-layout')).toBe('split')
    expect(halves.className).toContain('grid-cols-2')
    const left = screen.getByTestId('agents-left')
    const right = screen.getByTestId('agents-right')
    expect(headingsIn(left)).toEqual(['Starting agents', 'Launch presets'])
    expect(headingsIn(right)).toEqual(['Agent tools on this computer'])
    expect(left.className).toContain('overflow-y-auto')
    expect(right.className).toContain('overflow-y-auto')
    // The header stays put above both halves.
    const head = screen.getByRole('heading', { level: 2, name: 'Providers / Models' })
    expect(halves.contains(head)).toBe(false)
    expect(head.compareDocumentPosition(halves) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // Reading and keyboard order: the left half first.
    expect(left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(screen.getAllByRole('heading', { level: 3 }).map((h) => h.textContent)).toEqual([
      'Starting agents', 'Launch presets', 'Agent tools on this computer',
    ])
  })

  it(`narrow (< ${HALVES_MIN_WIDTH}px): the halves stack, the tools below, in one scroll`, () => {
    bodyWidth = HALVES_MIN_WIDTH - 1
    render(<AgentsSection />)
    const halves = screen.getByTestId('agents-halves')
    expect(halves.getAttribute('data-layout')).toBe('stacked')
    expect(halves.className).toContain('overflow-y-auto')
    expect(halves.className).not.toContain('grid-cols-2')
    const left = screen.getByTestId('agents-left')
    const right = screen.getByTestId('agents-right')
    expect(left.className).not.toContain('overflow-y-auto')
    expect(right.className).not.toContain('overflow-y-auto')
    expect(headingsIn(right)).toEqual(['Agent tools on this computer'])
    expect(left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('at 150% app zoom, compares the body’s CSS width: 900 px on screen is 600 CSS px, so the halves stack', () => {
    applyAppZoomForTest(1.5)
    bodyWidth = 900
    try {
      render(<AgentsSection />)
      expect(screen.getByTestId('agents-halves').getAttribute('data-layout')).toBe('stacked')
    } finally {
      resetAppZoomForTest()
    }
  })
})

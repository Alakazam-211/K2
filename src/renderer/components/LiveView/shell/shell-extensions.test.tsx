// @vitest-environment jsdom
// The shell's opt-in extensions for the editors (build-editors.md §4, E1b):
// a header with no state chip and a mode slot, an optional panel toggle with
// its own words, a frame with status and find-bar slots and a panel that
// reflows the text when narrow; and the hook the views share.

import { cleanup, render, renderHook, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import { shellSample } from '@/lib/live-view/shared/samples'
import { LiveHeader } from './LiveHeader'
import { LiveTabFrame } from './LiveTabFrame'
import { useLiveShell } from './useLiveShell'
import { SCENE_SHELL } from '../scene/scene-shell'

afterEach(cleanup)

describe('LiveHeader for a plain file', () => {
  it('no state chip, the path as the name’s tooltip, a mode slot, and no panel toggle unless asked', () => {
    render(<LiveHeader fileName="README.md" fileTitle="/ws/README.md" sentence={null} mode={<span data-testid="modes">modes</span>} />)
    expect(screen.queryByTestId('lv-state')).toBeNull()
    expect(screen.getByText('README.md').getAttribute('title')).toBe('/ws/README.md')
    expect(screen.getByTestId('modes')).toBeTruthy()
    expect(screen.queryByTestId('lv-panel-toggle')).toBeNull()
  })

  it('the mode slot sits after the view’s controls and before the panel toggle, which takes its own words', () => {
    render(<LiveHeader fileName="README.md" state="live" sentence={null} right={<span data-testid="right">r</span>} mode={<span data-testid="modes">m</span>} panelOpen={false} onTogglePanel={() => {}} panelLabel="Outline and Live" />)
    const order = Array.from(screen.getByTestId('lv-shell-header').querySelectorAll('[data-testid]')).map((e) => e.getAttribute('data-testid'))
    expect(order.indexOf('right')).toBeLessThan(order.indexOf('modes'))
    expect(order.indexOf('modes')).toBeLessThan(order.indexOf('lv-panel-toggle'))
    expect(screen.getByTestId('lv-panel-toggle').getAttribute('aria-label')).toBe('Show Outline and Live')
    expect(screen.getByTestId('lv-state').textContent).toBe('Live')
  })
})

describe('LiveTabFrame slots', () => {
  const frame = (p: Partial<React.ComponentProps<typeof LiveTabFrame>> = {}): void => {
    render(
      <LiveTabFrame
        header={<div>head</div>}
        notice={<div data-testid="notice" />}
        hero={<div data-testid="hero" />}
        panel={<div data-testid="panel-body" />}
        panelOpen
        narrow
        sample={false}
        onClosePanel={() => {}}
        {...p}
      />,
    )
  }

  it('a find bar under the notice, a status line under the hero', () => {
    frame({ findBar: <div data-testid="find" />, status: <div data-testid="status" /> })
    const all = Array.from(screen.getByTestId('lv-tab').querySelectorAll('[data-testid]')).map((e) => e.getAttribute('data-testid'))
    expect(all.indexOf('notice')).toBeLessThan(all.indexOf('find'))
    expect(all.indexOf('find')).toBeLessThan(all.indexOf('hero'))
    expect(all.indexOf('hero')).toBeLessThan(all.indexOf('status'))
    // The status line belongs to the text's column, not under the panel.
    expect(screen.getByTestId('status').closest('[data-testid="lv-panel"]')).toBeNull()
  })

  it('narrow: a sheet by default; reflow puts the panel beside the text, no sheet', () => {
    frame()
    expect(screen.getByTestId('lv-panel').hasAttribute('data-sheet')).toBe(true)
    cleanup()
    frame({ panelMode: 'reflow', panelLabel: 'Outline and Live' })
    const panel = screen.getByTestId('lv-panel')
    expect(panel.hasAttribute('data-sheet')).toBe(false)
    expect(panel.className).not.toContain('absolute')
    expect(panel.getAttribute('aria-label')).toBe('Outline and Live')
  })
})

describe('useLiveShell', () => {
  it('holds the view’s state over a source: narrow by the config, the panel, the tab state', () => {
    const src = new SampleShellSource(shellSample('scene'), { autoplay: false })
    const { result } = renderHook(() => useLiveShell(src, SCENE_SHELL, 800))
    expect(result.current.narrow).toBe(true)
    expect(result.current.panelOpen).toBe(false)
    expect(result.current.tabState).toBe('loading')
    expect(result.current.notice).toBeNull()
  })
})

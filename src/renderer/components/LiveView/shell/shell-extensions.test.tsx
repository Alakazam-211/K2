// @vitest-environment jsdom
// The shell's opt-in extensions for the editors (build-editors.md §4, E1b):
// a header with no state chip and a mode slot, an optional panel toggle with
// its own words, a frame with status and find-bar slots and a panel that
// reflows the text when narrow; and the hook the views share.

import { cleanup, fireEvent, render, renderHook, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import { shellSample } from '@/lib/live-view/shared/samples'
import { LiveHeader } from './LiveHeader'
import { LiveShellView } from './LiveShellView'
import { LiveTabFrame } from './LiveTabFrame'
import type { ShellHeroProps } from './shell-config'
import { useLiveShell } from './useLiveShell'
import { SCENE_SHELL } from '../scene/scene-shell'
import { SampleSceneSource } from '@/lib/live-view/scene/sample-source'

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

describe('a view that places the say row and the panel itself', () => {
  it('gets both as nodes, laid out where it puts them; the frame holds neither', () => {
    const src = new SampleShellSource(shellSample('scene'), { autoplay: false, scenario: 'people' })
    function Hero(p: ShellHeroProps): React.JSX.Element {
      return (
        <div data-testid="lv-hero">
          <div data-testid="upper">
            {p.say}
            {p.panel}
          </div>
          <div data-testid="lower" />
        </div>
      )
    }
    render(<LiveShellView source={src} config={{ ...SCENE_SHELL, sayAlways: true, placesSayAndPanel: true, Hero }} width={1440} />)
    const upper = screen.getByTestId('upper')
    expect(upper.contains(screen.getByTestId('lv-say'))).toBe(true)
    expect(upper.contains(screen.getByTestId('lv-panel'))).toBe(true)
    expect(screen.getAllByTestId('lv-panel')).toHaveLength(1)
    expect(screen.getAllByTestId('lv-say')).toHaveLength(1)
    // the toggle still opens and closes it
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    expect(screen.queryByTestId('lv-panel')).toBeNull()
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

describe('a refusal the view draws on its picture', () => {
  const refused = (): SampleSceneSource => {
    const src = new SampleSceneSource({ autoplay: false, scenario: 'refuse' })
    src.player.goTo(1)
    return src
  }
  function Hero(p: ShellHeroProps): React.JSX.Element {
    const r = p.refusal
    return (
      <div data-testid="lv-hero">
        {r ? (
          <div data-testid="callout">
            {r.view.title}
            {r.view.actions
              .filter((a) => a.id === 'ask')
              .map((a) => (
                <button key={a.id} type="button" onClick={() => r.onAction(a.id)}>
                  {a.label}
                </button>
              ))}
          </div>
        ) : null}
      </div>
    )
  }

  it('isn’t in the notice bar; the view gets the shell’s words and actions', () => {
    render(<LiveShellView source={refused()} config={{ ...SCENE_SHELL, refusalOnPicture: true, Hero }} width={1440} />)
    expect(screen.queryByTestId('lv-notice')).toBeNull()
    expect(screen.getByTestId('callout').textContent).toContain('Brook couldn’t rename Chair.002: Atlas is holding it.')
  })

  it('Ask opens the say row to the holder, about the object that was refused', () => {
    render(<LiveShellView source={refused()} config={{ ...SCENE_SHELL, refusalOnPicture: true, Hero }} width={1440} />)
    expect(screen.queryByTestId('lv-say')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Ask Atlas' }))
    expect(screen.getByTestId('lv-say')).toBeTruthy()
    expect(screen.getByRole('textbox', { name: /Atlas/ })).toBeTruthy()
    expect(screen.getByTestId('lv-say-chip').textContent).toContain('Chair.002')
    // Opened to ask (not by following): it closes.
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(screen.queryByTestId('lv-say')).toBeNull()
  })

  it('a view that doesn’t draw it keeps it in the notice bar', () => {
    render(<LiveShellView source={refused()} config={{ ...SCENE_SHELL, refusalOnPicture: false, Hero }} width={1440} />)
    expect(screen.getByTestId('lv-notice').getAttribute('data-kind')).toBe('refused')
    expect(screen.queryByTestId('callout')).toBeNull()
  })
})

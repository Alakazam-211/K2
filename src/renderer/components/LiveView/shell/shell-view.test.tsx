// @vitest-environment jsdom
// The shell put together over the sample source (both tabs): first open
// under the "Sample data" banner, Follow and Stop, Watching and Changes, one
// notice at a time, each connection state said once, the off-screen stack.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import { shellSample } from '@/lib/live-view/shared/samples'
import { LiveShellView } from './LiveShellView'
import { SCENE_SHELL } from '../scene/scene-shell'
import { TIMELINE_SHELL } from '../timeline/timeline-shell'

beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }))
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

function mount(kind: 'scene' | 'timeline', scenario?: string): SampleShellSource {
  const src = new SampleShellSource(shellSample(kind), { autoplay: false })
  if (scenario) src.player.setScenario(scenario)
  render(<LiveShellView source={src} config={kind === 'scene' ? SCENE_SHELL : TIMELINE_SHELL} width={1440} />)
  return src
}

describe('first open', () => {
  it.each(['scene', 'timeline'] as const)('%s: the shell in place under the sample banner, the content area a placeholder', (kind) => {
    mount(kind)
    expect(screen.getByTestId('lv-sample-banner').textContent).toContain('Sample data, not this file')
    expect(screen.getByTestId('lv-shell-header')).toBeTruthy()
    expect(screen.getByTestId('lv-state').textContent).toBe('Loading')
    expect(screen.getByTestId('lv-hero').getAttribute('data-kind')).toBe(kind)
    expect(screen.getByTestId('lv-tab').getAttribute('data-sample')).toBe('')
  })
})

describe('people and follow', () => {
  it('Follow Atlas, then “Following Atlas · Stop”, then Stop', () => {
    mount('timeline', 'people')
    // The header's Follow (the stack's Atlas is named "Follow Atlas" too).
    expect(screen.getAllByRole('button', { name: 'Follow Atlas' })).toHaveLength(2)
    fireEvent.click(screen.getByTestId('lv-follow'))
    const following = screen.getByRole('button', { name: 'Following Atlas · Stop' })
    expect(following.getAttribute('aria-pressed')).toBe('true')
    expect(screen.getByText('following Atlas')).toBeTruthy()
    fireEvent.click(following)
    expect(screen.getByTestId('lv-follow').textContent).toBe('Follow Atlas')
  })

  it('click anyone in the stack to follow them; the Say row speaks to whom you follow', () => {
    mount('scene', 'people')
    const stack = screen.getByRole('group', { name: 'Who’s here: click someone to follow them' })
    fireEvent.click(within(stack).getByRole('button', { name: 'Follow Brook' }))
    expect(screen.getByRole('button', { name: 'Following Brook · Stop' })).toBeTruthy()
    expect(screen.getByRole('textbox', { name: /^Say something to Brook/ })).toBeTruthy()
  })

  it('Watching counts only who’s here; Changes lists the changes', () => {
    const src = mount('timeline', 'people')
    const here = src.snapshot().people.filter((p) => !p.away).length
    expect(screen.getByTestId('lv-watching-count').textContent).toBe(String(here))
    expect(screen.getByTestId('lv-away')).toBeTruthy()
    expect(screen.getAllByTestId('lv-change').length).toBe(src.snapshot().changes.length)
  })

  it('the off-screen stack jumps to the people it holds', () => {
    mount('timeline', 'people')
    const jump = screen.getByRole('button', { name: /off screen to the right: go there$/ })
    fireEvent.click(jump)
    expect(screen.getByTestId('lv-hero').getAttribute('data-jumped')).toBe('right')
  })

  it('the panel closes and opens from the header', () => {
    mount('timeline', 'people')
    fireEvent.click(screen.getByRole('button', { name: 'Hide the side panel' }))
    expect(screen.queryByTestId('lv-panel')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Show the side panel' }))
    expect(screen.getByTestId('lv-panel')).toBeTruthy()
  })

  it('a narrow tab starts with the panel closed and opens it as a sheet', () => {
    const src = new SampleShellSource(shellSample('timeline'), { autoplay: false })
    src.player.setScenario('people')
    render(<LiveShellView source={src} config={TIMELINE_SHELL} width={800} />)
    expect(screen.queryByTestId('lv-panel')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Show the side panel' }))
    expect(screen.getByTestId('lv-panel').getAttribute('data-sheet')).toBe('')
  })
})

describe('notices', () => {
  it('each connection state shows once', () => {
    const src = mount('timeline', 'connection')
    const words: Record<string, string> = { reconnecting: 'Reconnecting…', trying: 'Trying again now…', cant: 'Can’t reach this file’s server.', offline: 'You’re offline.', back: 'Back.' }
    for (const state of ['reconnecting', 'trying', 'cant', 'offline', 'back']) {
      expect(src.snapshot().connection).toBe(state)
      expect(screen.getAllByTestId('lv-notice')).toHaveLength(1)
      expect(screen.getAllByText(words[state])).toHaveLength(1)
      act(() => src.player.step())
    }
  })

  it('a connection outranks an outside save; a dismissed save steps aside', () => {
    const src = mount('timeline', 'saves')
    act(() => src.player.step())
    expect(screen.getByTestId('lv-notice').textContent).toContain('Saved outside K2: that save is now live.')
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    expect(screen.queryByTestId('lv-notice')).toBeNull()
  })

  it('Retry now moves the sample on to Trying…', () => {
    mount('scene', 'connection')
    fireEvent.click(screen.getByRole('button', { name: 'Retry now' }))
    expect((screen.getByRole('button', { name: 'Trying…' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('while not connected, the say field is off and Watching reads as you last saw', () => {
    mount('timeline', 'connection')
    expect((screen.getByRole('textbox') as HTMLInputElement).placeholder).toBe('Back online to send')
    expect(screen.getByTestId('lv-watching-count').textContent).toBe('as you last saw')
    expect(screen.getByTestId('lv-state').textContent).toBe('Reconnecting')
  })
})

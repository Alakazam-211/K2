// @vitest-environment jsdom
// Every remaining state of the scene tab (S6), as the tab says it and what
// each offers next, on the shared shell's pieces: a large scene (the plain
// line and Find an object), too large (the saved version, Open saved
// versions), empty (Ask Atlas), the connection, the four outside saves, a
// refusal and the hold released, without modifiers, and the 800 px panel.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { SampleSceneSource } from '@/lib/live-view/scene/sample-source'
import { LiveShellView } from '../shell/LiveShellView'
import { SCENE_SHELL } from './scene-shell'
import { LiveEntrySwitchProvider, type LiveEntrySwitchValue } from '../LiveEntrySwitch'

let src: SampleSceneSource | null = null
afterEach(() => {
  cleanup()
  src?.close()
})

function mount(scenario: string, step = 0, width = 1440): SampleSceneSource {
  src = new SampleSceneSource({ autoplay: false, scenario })
  src.player.goTo(step)
  render(<LiveShellView source={src} config={SCENE_SHELL} width={width} />)
  return src
}
const go = (step: number): void => act(() => src!.player.goTo(step))
const notice = (): HTMLElement | null => screen.queryByTestId('lv-notice')
const actions = (): string[] => [...(notice()?.querySelectorAll('button') ?? [])].map((b) => b.textContent ?? '')
const activity = (): string => screen.getByTestId('lv-scene-section-activity').textContent ?? ''

describe('a large scene', () => {
  it('says it is large in a plain line and offers Find an object', () => {
    mount('large', 0)
    expect(notice()?.getAttribute('data-kind')).toBe('size:large')
    expect(notice()?.textContent).toContain('A large scene: 2,000 objects in 100 collections.')
    expect(notice()?.textContent).toContain('Collections start closed and the list draws only what fits.')
    expect(actions()).toEqual(['Find an object', 'Dismiss'])
  })

  it('Find an object opens the panel and puts you in the find field', async () => {
    mount('large', 0, 800)
    expect(screen.queryByTestId('lv-panel')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Find an object' }))
    expect(screen.getByTestId('lv-panel')).toBeTruthy()
    await waitFor(() => expect(document.activeElement?.getAttribute('aria-label')).toBe('Find an object by name'))
  })

  it('the plain line is for opening: a later step doesn’t repeat it', () => {
    mount('large', 1)
    expect(notice()).toBeNull()
  })
})

describe('too large to watch live', () => {
  it('the saved version, read-only, with Open the saved file', () => {
    mount('blob')
    expect(notice()?.getAttribute('data-kind')).toBe('size:too_large')
    expect(notice()?.textContent).toContain('Too large to watch live')
    expect(notice()?.textContent).toContain('This scene has 2,600 objects; live editing stops at about 2,000. You see the last saved version, read-only.')
    expect(actions()).toEqual(['Open the saved file'])
    expect(screen.getByTestId('lv-state').getAttribute('data-state')).toBe('read-only')
  })

  it('Open the saved file switches the tab’s File | Live to File', () => {
    const onMode = vi.fn()
    const entry: LiveEntrySwitchValue = { group: 'Scene view', testIdPrefix: 'live-scene', hint: '', mode: 'live', onMode }
    src = new SampleSceneSource({ autoplay: false, scenario: 'blob' })
    render(
      <LiveEntrySwitchProvider value={entry}>
        <LiveShellView source={src} config={SCENE_SHELL} width={1440} />
      </LiveEntrySwitchProvider>,
    )
    fireEvent.click(within(notice()!).getByRole('button', { name: 'Open the saved file' }))
    expect(onMode).toHaveBeenCalledWith('file')
  })
})

describe('an empty scene', () => {
  it('says where objects come from; Ask Atlas opens the say row to Atlas about the scene', () => {
    mount('empty')
    const card = screen.getByTestId('lv-scene-card')
    expect(card.textContent).toContain('No objects in this scene')
    expect(screen.queryByTestId('lv-say')).toBeNull()
    fireEvent.click(within(card).getByRole('button', { name: 'Ask Atlas' }))
    expect(screen.getByRole('textbox', { name: /Say something to Atlas/ })).toBeTruthy()
    expect(screen.getByTestId('lv-say-chip').textContent).toContain('the scene')
  })
})

describe('the connection', () => {
  it('reconnecting (Retry now), gave up (Retry, Open the saved file), back', () => {
    mount('lost', 1)
    expect(notice()?.getAttribute('data-kind')).toBe('connection:reconnecting')
    expect(actions()).toEqual(['Retry now'])
    go(2)
    expect(notice()?.getAttribute('data-kind')).toBe('connection:cant')
    expect(actions()).toEqual(['Retry', 'Open the saved file'])
    go(3)
    expect(notice()?.textContent).toContain('Nothing changed while you were away.')
  })
})

describe('the four outside saves', () => {
  it.each([
    ['O1', 1, 'save:ingested', 'Saved outside K2 · now in the live file', ['Dismiss']],
    ['O2', 1, 'save:kept_both', 'Saved outside K2: that save is now live.', ['Open the copy', 'History', 'Dismiss']],
    ['O4', 1, 'save:declined', 'Saved outside K2, and the file can’t be live any more.', ['Open the saved file']],
    ['O5', 1, 'save:missing', 'was moved or deleted outside K2.', ['Save it again']],
  ])('%s: %s', (id, step, kind, words, acts) => {
    mount(id, step)
    expect(notice()?.getAttribute('data-kind')).toBe(kind)
    expect(notice()?.textContent).toContain(words)
    expect(actions()).toEqual(acts)
  })

  it('O3 (a new object) is taken in, like O1', () => {
    mount('O3', 1)
    expect(notice()?.getAttribute('data-kind')).toBe('save:ingested')
  })
})

describe('a refusal, then the hold released', () => {
  it('the callout on the picture, not the bar; then Activity says Atlas let go and was holding it', () => {
    mount('refuse', 1)
    expect(notice()).toBeNull()
    expect(activity()).toContain('Brook couldn’t rename Chair.002: Atlas is holding it')
    go(2)
    expect(activity()).toContain('Atlas let go of the dining set')
    expect(activity()).toContain('Atlas was holding it')
  })
})

describe('the 800 px panel', () => {
  it('starts closed and opens as a sheet over the picture', () => {
    mount('refuse', 1, 800)
    expect(screen.queryByTestId('lv-panel')).toBeNull()
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    expect(screen.getByTestId('lv-panel').hasAttribute('data-sheet')).toBe(true)
  })
})

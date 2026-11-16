// @vitest-environment jsdom
// "Open the saved file" works (shell, for every live tab): from the notice
// bar or from a card in the view, it flips the tab's File | Live switch to
// File, which shows the file as saved.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { setLivePreview } from '@/lib/live-view/shared/preview-keys'
import { LivePreviewEntry } from '../LivePreviewEntry'

beforeEach(() => window.localStorage.clear())
afterEach(() => cleanup())

async function watchTimeline(): Promise<void> {
  setLivePreview('timeline', true)
  render(
    <LivePreviewEntry filePath="/ws/edits/rough-cut.otio">
      <div data-testid="file-content">the file</div>
    </LivePreviewEntry>,
  )
  fireEvent.click(screen.getByTestId('live-timeline-watch'))
  await screen.findByTestId('lv-tab', {}, { timeout: 5000 })
  await act(async () => {})
}
function pick(id: string): void {
  fireEvent.click(within(screen.getByTestId('lv-sample-scenarios')).getByRole('button'))
  fireEvent.click(document.querySelector(`[data-setting-dropdown-menu] button[data-value="${id}"]`) as HTMLElement)
}

describe('Open the saved file', () => {
  it('a card’s primary action flips the tab to File', async () => {
    await watchTimeline()
    pick('errors')
    const card = screen.getByTestId('lv-mon-stage')
    fireEvent.click(within(card).getByRole('button', { name: 'Open the saved file' }))
    expect(screen.getByTestId('file-content')).toBeTruthy()
    expect(screen.queryByTestId('lv-tab')).toBeNull()
  })

  it('the notice bar’s button does the same', async () => {
    await watchTimeline()
    pick('save-declined')
    fireEvent.click(screen.getByTestId('lv-sample-step'))
    fireEvent.click(within(screen.getByTestId('lv-notice')).getByRole('button', { name: 'Open the saved file' }))
    expect(screen.getByTestId('file-content')).toBeTruthy()
    expect(screen.queryByTestId('lv-tab')).toBeNull()
  })
})

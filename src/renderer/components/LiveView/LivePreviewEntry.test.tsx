// @vitest-environment jsdom
// The per-type way in: each preview switch gates its own type. Off, a tab
// is exactly as before; on, it offers "Watch live (preview)", which mounts
// that type's sample view and hides the any-file panel. And the Settings
// rows that flip them.

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { readLivePreview, setLivePreview, switchFor } from '@/lib/live-view/shared/preview-keys'
import { LivePreviewEntry } from './LivePreviewEntry'
import { LivePreviewSwitches } from './LivePreviewSwitches'

const content = <div data-testid="file-content">the file</div>

beforeEach(() => window.localStorage.clear())
afterEach(() => cleanup())

describe('LivePreviewEntry', () => {
  it('with every switch off, every type renders the file view and nothing else', () => {
    for (const path of ['/ws/budget.xlsx', '/ws/living-room.blend', '/ws/edits/rough-cut.otio']) {
      const { unmount } = render(<LivePreviewEntry filePath={path}>{content}</LivePreviewEntry>)
      expect(screen.getByTestId('file-content')).toBeTruthy()
      expect(screen.queryByText('Watch live (preview)')).toBeNull()
      unmount()
    }
  })

  it('each switch gates only its own type', () => {
    setLivePreview('scene', true)
    const { unmount } = render(<LivePreviewEntry filePath="/ws/living-room.blend">{content}</LivePreviewEntry>)
    expect(screen.getByTestId('live-scene-entry')).toBeTruthy()
    unmount()
    const t = render(<LivePreviewEntry filePath="/ws/edits/rough-cut.otio">{content}</LivePreviewEntry>)
    expect(screen.queryByTestId('live-timeline-entry')).toBeNull()
    t.unmount()
    const w = render(<LivePreviewEntry filePath="/ws/budget.xlsx">{content}</LivePreviewEntry>)
    expect(screen.queryByTestId('live-workbook-entry')).toBeNull()
    w.unmount()
  })

  it.each([
    ['scene', '/ws/living-room.blend', 'Plays K2’s sample scene, not this file.'],
    ['timeline', '/ws/edits/rough-cut.otio', 'Plays K2’s sample edit, not this file.'],
  ] as const)('%s: Watch live mounts its sample tab under the banner; File goes back; the any-file panel hides while live', async (type, path, hint) => {
    setLivePreview(type, true)
    const onLive = vi.fn()
    render(
      <LivePreviewEntry filePath={path} onLiveChange={onLive}>
        {content}
      </LivePreviewEntry>,
    )
    fireEvent.click(screen.getByTestId(`live-${type}-watch`))
    expect(screen.getByText(hint)).toBeTruthy()
    expect(await screen.findByTestId('lv-tab', {}, { timeout: 5000 })).toBeTruthy()
    expect(screen.getByTestId('lv-sample-banner').textContent).toContain('Sample data, not this file')
    expect(screen.queryByTestId('file-content')).toBeNull()
    expect(onLive).toHaveBeenLastCalledWith(true)
    fireEvent.click(screen.getByRole('radio', { name: 'File' }))
    expect(screen.getByTestId('file-content')).toBeTruthy()
    expect(onLive).toHaveBeenLastCalledWith(false)
  })

  it('turning a switch off in another window takes it away', () => {
    setLivePreview('timeline', true)
    render(<LivePreviewEntry filePath="/ws/edits/rough-cut.otio">{content}</LivePreviewEntry>)
    expect(screen.getByTestId('live-timeline-entry')).toBeTruthy()
    act(() => {
      window.localStorage.removeItem(switchFor('timeline').storageKey)
      window.dispatchEvent(new StorageEvent('storage', { key: switchFor('timeline').storageKey }))
    })
    expect(screen.queryByTestId('live-timeline-entry')).toBeNull()
  })
})

describe('Settings → Multiplayer: the preview switches', () => {
  it('one switch per type, beside the workbook’s, each flipping only its own key', () => {
    render(<LivePreviewSwitches />)
    const scene = screen.getByTestId('live-scene-preview-toggle')
    const timeline = screen.getByTestId('live-timeline-preview-toggle')
    expect(screen.getByTestId('live-workbook-preview-toggle')).toBeTruthy()
    expect(screen.getByText('Watch 3D scenes live (preview)')).toBeTruthy()
    expect(screen.getByText('Watch timelines live (preview)')).toBeTruthy()
    expect(scene.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(scene)
    expect(readLivePreview('scene')).toBe(true)
    expect(readLivePreview('timeline')).toBe(false)
    expect(readLivePreview('workbook')).toBe(false)
    expect(screen.getByTestId('live-scene-preview-toggle').getAttribute('aria-checked')).toBe('true')
    fireEvent.click(timeline)
    expect(readLivePreview('timeline')).toBe(true)
  })
})

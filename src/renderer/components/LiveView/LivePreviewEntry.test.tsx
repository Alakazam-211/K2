// @vitest-environment jsdom
// The per-type way in: each preview switch gates its own type. Off, a tab
// is exactly as before; on, it offers "Watch live (preview)", which mounts
// that type's sample view and hides the any-file panel. And the Settings
// rows that flip them.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { readLivePreview, setLivePreview, switchFor } from '@/lib/live-view/shared/preview-keys'
import { LiveEntrySwitchProvider } from './LiveEntrySwitch'
import { LivePreviewEntry } from './LivePreviewEntry'
import { LiveHeader } from './shell/LiveHeader'
import { LivePreviewSwitches } from './LivePreviewSwitches'

// A real timeline's socket that never answers: the tab waits on its spinner (TimelineLive's own tests script one).
const dialled: string[] = []
vi.mock('@/lib/docs-live-daemon', () => ({
  createDaemonLiveTransport: (_scope: unknown, doc: { path: string }) => {
    dialled.push(doc.path)
    return { open: false, connect: () => new Promise(() => {}), send: () => {}, onFrame: () => () => {}, onClose: () => () => {}, close: () => {} }
  },
}))

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
    expect(screen.getByText(hint)).toBeTruthy()
    fireEvent.click(screen.getByTestId(`live-${type}-watch`))
    expect(await screen.findByTestId('lv-tab', {}, { timeout: 5000 })).toBeTruthy()
    expect(screen.getByTestId('lv-sample-banner').textContent).toContain('Sample data, not this file')
    expect(screen.queryByTestId('file-content')).toBeNull()
    expect(onLive).toHaveBeenLastCalledWith(true)
    fireEvent.click(screen.getByRole('radio', { name: 'File' }))
    expect(screen.getByTestId('file-content')).toBeTruthy()
    expect(onLive).toHaveBeenLastCalledWith(false)
  })

  it('a timeline the daemon serves watches the file itself: the switch shows with the preview key off, and Live has no Sample data banner', async () => {
    const scope = { id: 'local' } as never
    dialled.length = 0
    render(
      <LivePreviewEntry filePath="/ws/edits/cut.otio" real={scope}>
        {content}
      </LivePreviewEntry>,
    )
    expect(screen.getByText('Watches this file live: changes, playheads and who’s here.')).toBeTruthy()
    fireEvent.click(screen.getByTestId('live-timeline-watch'))
    expect(await screen.findByTestId('lv-tab', {}, { timeout: 5000 })).toBeTruthy()
    expect(screen.getByTestId('live-timeline-entry').getAttribute('data-source')).toBe('real')
    expect(screen.queryByTestId('lv-sample-banner')).toBeNull()
    expect(dialled).toEqual(['/ws/edits/cut.otio'])
  })

  it('served or not, a type with no real view yet plays its sample (a workbook); a served scene has its real view (S8); a timeline not served plays the sample', async () => {
    const scope = { id: 'local' } as never
    setLivePreview('workbook', true)
    const w = render(<LivePreviewEntry filePath="/ws/budget.xlsx" real={scope}>{content}</LivePreviewEntry>)
    expect(screen.getByTestId('live-workbook-entry').getAttribute('data-source')).toBe('sample')
    w.unmount()
    const s = render(<LivePreviewEntry filePath="/ws/living-room.blend" real={scope}>{content}</LivePreviewEntry>)
    expect(screen.getByTestId('live-scene-entry').getAttribute('data-source')).toBe('real')
    s.unmount()
    render(<LivePreviewEntry filePath="/ws/edits/cut.otio" real={null}>{content}</LivePreviewEntry>)
    expect(screen.queryByTestId('live-timeline-entry')).toBeNull()
  })

  it.each([
    ['scene', '/ws/living-room.blend', 'Scene view', 'lv-shell-header'],
    ['timeline', '/ws/edits/rough-cut.otio', 'Timeline view', 'lv-shell-header'],
    ['workbook', '/ws/budget.xlsx', 'Workbook view', 'lv-header'],
  ] as const)('%s: while watching, File | Watch live sits once in the live header, with no row of its own above it', async (type, path, group, header) => {
    setLivePreview(type, true)
    render(<LivePreviewEntry filePath={path}>{content}</LivePreviewEntry>)
    expect(screen.getByTestId(`live-${type}-switch-row`)).toBeTruthy()
    fireEvent.click(screen.getByTestId(`live-${type}-watch`))
    const head = await screen.findByTestId(header, {}, { timeout: 5000 })
    expect(screen.queryByTestId(`live-${type}-switch-row`)).toBeNull()
    expect(screen.getAllByRole('radiogroup', { name: group })).toHaveLength(1)
    const sw = within(head).getByRole('radiogroup', { name: group })
    // At the header's right end, before the panel toggle (the scene and timeline).
    const toggle = within(head).queryByTestId('lv-panel-toggle')
    if (toggle) expect(sw.compareDocumentPosition(toggle) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    fireEvent.click(within(head).getByRole('radio', { name: 'File' }))
    expect(screen.getByTestId('file-content')).toBeTruthy()
    expect(screen.getByTestId(`live-${type}-switch-row`)).toBeTruthy()
  })

  it('the header has one mode area: the view’s modes, then the file tab’s switch, then the panel toggle; with no file tab around it, no switch', () => {
    const sw = { group: 'Scene view', hint: 'h', testIdPrefix: 'live-scene', mode: 'live' as const, onMode: () => {} }
    const head = (): React.JSX.Element => <LiveHeader fileName="x" sentence={null} mode={<span data-testid="modes">m</span>} panelOpen={false} onTogglePanel={() => {}} />
    const { unmount } = render(<LiveEntrySwitchProvider value={sw}>{head()}</LiveEntrySwitchProvider>)
    const order = Array.from(screen.getByTestId('lv-shell-header').querySelectorAll('[data-testid]')).map((e) => e.getAttribute('data-testid'))
    expect(order.indexOf('modes')).toBeLessThan(order.indexOf('live-scene-switch'))
    expect(order.indexOf('live-scene-switch')).toBeLessThan(order.indexOf('lv-panel-toggle'))
    unmount()
    render(head())
    expect(screen.queryByTestId('live-scene-switch')).toBeNull()
    expect(screen.queryByRole('radio', { name: 'File' })).toBeNull()
  })

  it('the sample banner is one row: its words never wrap', async () => {
    setLivePreview('scene', true)
    render(<LivePreviewEntry filePath="/ws/living-room.blend">{content}</LivePreviewEntry>)
    fireEvent.click(screen.getByTestId('live-scene-watch'))
    const banner = await screen.findByTestId('lv-sample-banner', {}, { timeout: 5000 })
    expect(banner.getAttribute('data-one-row')).toBe('')
    expect(banner.className).not.toContain('flex-wrap')
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

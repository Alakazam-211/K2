// @vitest-environment jsdom
// The owner's key gates the way in: off, an .xlsx tab is exactly as before;
// on, it offers "Watch live (preview)", which mounts the sample-driven view.

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { LS_LIVE_WORKBOOK_PREVIEW, readLiveWorkbookPreview, setLiveWorkbookPreview } from '@/lib/live-view/preview-key'
import { LiveWorkbookEntry, WATCH_LIVE_LABEL } from './LiveWorkbookEntry'
import { LiveWorkbookPreviewGroup } from './LiveWorkbookPreviewRow'

const content = <div data-testid="file-content">the file</div>

beforeEach(() => window.localStorage.clear())
afterEach(() => cleanup())

describe('LiveWorkbookEntry', () => {
  it('with the key off (the default), renders the file view and nothing else', () => {
    expect(readLiveWorkbookPreview()).toBe(false)
    render(<LiveWorkbookEntry filePath="/ws/budget.xlsx">{content}</LiveWorkbookEntry>)
    expect(screen.getByTestId('file-content')).toBeTruthy()
    expect(screen.queryByTestId('live-workbook-entry')).toBeNull()
    expect(screen.queryByText(WATCH_LIVE_LABEL)).toBeNull()
  })

  it('with the key on, only .xlsx files offer it', () => {
    setLiveWorkbookPreview(true)
    render(<LiveWorkbookEntry filePath="/ws/notes.csv">{content}</LiveWorkbookEntry>)
    expect(screen.queryByTestId('live-workbook-entry')).toBeNull()
  })

  it('with the key on, "Watch live (preview)" mounts the view with the sample banner; "File" goes back', async () => {
    setLiveWorkbookPreview(true)
    render(<LiveWorkbookEntry filePath="/ws/Budget.XLSX">{content}</LiveWorkbookEntry>)
    expect(screen.getByTestId('file-content')).toBeTruthy()
    fireEvent.click(screen.getByTestId('live-workbook-watch'))
    expect(await screen.findByTestId('live-workbook-view', {}, { timeout: 5000 })).toBeTruthy()
    expect(screen.getByTestId('lv-sample-banner').textContent).toContain('Sample data, not this file')
    expect(screen.queryByTestId('file-content')).toBeNull()
    fireEvent.click(screen.getByRole('radio', { name: 'File' }))
    expect(screen.getByTestId('file-content')).toBeTruthy()
  })

  it('turning the key off elsewhere (another window) takes the switch away', () => {
    setLiveWorkbookPreview(true)
    render(<LiveWorkbookEntry filePath="/ws/budget.xlsx">{content}</LiveWorkbookEntry>)
    expect(screen.getByTestId('live-workbook-entry')).toBeTruthy()
    act(() => {
      window.localStorage.removeItem(LS_LIVE_WORKBOOK_PREVIEW)
      window.dispatchEvent(new StorageEvent('storage', { key: LS_LIVE_WORKBOOK_PREVIEW }))
    })
    expect(screen.queryByTestId('live-workbook-entry')).toBeNull()
  })
})

describe('the Settings switch', () => {
  it('turns the key on and off', () => {
    render(<LiveWorkbookPreviewGroup />)
    const sw = screen.getByTestId('live-workbook-preview-toggle')
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    expect(window.localStorage.getItem(LS_LIVE_WORKBOOK_PREVIEW)).toBe('1')
    expect(screen.getByTestId('live-workbook-preview-toggle').getAttribute('aria-checked')).toBe('true')
    fireEvent.click(screen.getByTestId('live-workbook-preview-toggle'))
    expect(readLiveWorkbookPreview()).toBe(false)
  })
})

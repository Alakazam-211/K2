// "Copy path": the file a tab stands for, and the copy (the Tauri clipboard
// plugin first, then navigator.clipboard; a toast either way).

import { beforeEach, describe, expect, it, vi } from 'vitest'

const clip = vi.hoisted(() => ({ writeText: vi.fn(async (_text: string) => {}) }))
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  writeText: (text: string) => clip.writeText(text),
}))

import { copyPathWithToast, tabFilePath } from '@/lib/copy-path'
import type { Item, PaneGroup } from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'

function file(path: string): Item {
  return { id: `f-${path}`, type: 'file-viewer', data: { filePath: path }, pinned: false }
}
const terminal: Item = { id: 't', type: 'terminal', data: { terminalId: 't', cwd: '/w' } }

function tab(...groups: Array<[Item[], number]>): { paneGroups: Map<string, PaneGroup> } {
  return {
    paneGroups: new Map(groups.map(([items, activeItemIndex], i) => [`pg${i}`, { id: `pg${i}`, items, activeItemIndex }])),
  }
}

describe('tabFilePath', () => {
  it("is the pane's selected file", () => {
    expect(tabFilePath(tab([[file('/w/a.md'), file('/w/b.md')], 1]))).toBe('/w/b.md')
    expect(tabFilePath(tab([[file('/w/a.md'), file('/w/b.md')], 0]))).toBe('/w/a.md')
  })

  it('is null when no pane shows a file', () => {
    expect(tabFilePath(tab([[terminal], 0]))).toBeNull()
    expect(tabFilePath(tab([[terminal, file('/w/a.md')], 0]))).toBeNull()
    expect(tabFilePath(tab([[file('')], 0]))).toBeNull()
  })

  it('is the first pane showing a file in a split tab', () => {
    expect(tabFilePath(tab([[terminal], 0], [[file('/w/plan.dxf')], 0]))).toBe('/w/plan.dxf')
  })
})

describe('copyPathWithToast', () => {
  const webWrite = vi.fn(async (_text: string) => {})

  beforeEach(() => {
    useToastStore.setState({ toasts: [] })
    clip.writeText.mockReset()
    clip.writeText.mockImplementation(async () => {})
    webWrite.mockReset()
    webWrite.mockImplementation(async () => {})
    Object.defineProperty(globalThis, 'navigator', { value: { clipboard: { writeText: webWrite } }, configurable: true })
  })

  it('writes through the clipboard plugin and says "Path copied"', async () => {
    await copyPathWithToast('/w/plan.dxf')
    expect(clip.writeText).toHaveBeenCalledWith('/w/plan.dxf')
    expect(webWrite).not.toHaveBeenCalled()
    expect(useToastStore.getState().toasts.map((t) => [t.type, t.message])).toEqual([['success', 'Path copied']])
  })

  it('falls back to the web clipboard without the plugin (the web client)', async () => {
    clip.writeText.mockImplementation(async () => {
      throw new Error('no plugin')
    })
    await copyPathWithToast('/w/plan.dxf')
    expect(webWrite).toHaveBeenCalledWith('/w/plan.dxf')
    expect(useToastStore.getState().toasts.map((t) => t.message)).toEqual(['Path copied'])
  })

  it('says why when both fail', async () => {
    clip.writeText.mockImplementation(async () => {
      throw new Error('no plugin')
    })
    webWrite.mockImplementation(async () => {
      throw new Error('not allowed')
    })
    await copyPathWithToast('/w/plan.dxf')
    expect(useToastStore.getState().toasts.map((t) => [t.type, t.message])).toEqual([
      ['error', "Couldn't copy the path: not allowed"],
    ])
  })
})

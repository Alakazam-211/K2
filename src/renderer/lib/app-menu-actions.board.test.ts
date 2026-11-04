// Quit from K2's own menu (Windows and Linux) asks about a board file's unsaved edit first.
import { beforeEach, describe, expect, it, vi } from 'vitest'

const exit = vi.hoisted(() => vi.fn(async () => undefined))
const ask = vi.hoisted(() => vi.fn(async () => true))
vi.mock('@tauri-apps/plugin-process', () => ({ exit }))
vi.mock('@/lib/board-file/board-file-tabs', () => ({ askBeforeBoardFileLoss: ask }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({}) }))
vi.mock('@tauri-apps/api/webviewWindow', () => ({ getCurrentWebviewWindow: () => ({}) }))

import { handleAppMenuAction } from './app-menu-actions'

describe('Quit in K2’s menu', () => {
  beforeEach(() => {
    exit.mockClear()
    ask.mockClear()
  })

  it('stays when the person says No', async () => {
    ask.mockResolvedValueOnce(false)
    await handleAppMenuAction('quit')
    expect(ask).toHaveBeenCalledWith('quit')
    expect(exit).not.toHaveBeenCalled()
  })

  it('quits when nothing is unsaved, or the person says Yes', async () => {
    await handleAppMenuAction('quit')
    expect(exit).toHaveBeenCalledWith(0)
  })
})

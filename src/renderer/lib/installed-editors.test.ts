import { describe, expect, it, vi, beforeEach } from 'vitest'

const h = vi.hoisted(() => ({ reply: null as unknown, fail: false }))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => {
    if (h.fail) throw new Error('daemon unreachable')
    return h.reply
  }),
}))

import { defaultEditorFirst, installedEditorsDefaultFirst } from './installed-editors'
import { useSettingsStore } from '@/stores/settings'

const LIST = [
  { id: 'cursor', label: 'Cursor' },
  { id: 'vscode', label: 'VS Code' },
  { id: 'zed', label: 'Zed' },
]

describe('defaultEditorFirst', () => {
  it('moves the picked editor to the front and keeps the rest in order', () => {
    expect(defaultEditorFirst(LIST, 'Zed').map((e) => e.id)).toEqual(['zed', 'cursor', 'vscode'])
  })
  it('leaves the list alone when nothing is picked, the pick is first, or it is not installed', () => {
    expect(defaultEditorFirst(LIST, undefined)).toBe(LIST)
    expect(defaultEditorFirst(LIST, 'Cursor')).toBe(LIST)
    expect(defaultEditorFirst(LIST, 'Sublime Text')).toBe(LIST)
  })
})

describe('installedEditorsDefaultFirst', () => {
  beforeEach(() => {
    h.fail = false
    h.reply = LIST
    useSettingsStore.setState({ projectSettings: { __global__: { defaultEditor: 'VS Code' } } })
  })
  it('reads the default from Settings → Defaults', async () => {
    expect((await installedEditorsDefaultFirst()).map((e) => e.id)).toEqual(['vscode', 'cursor', 'zed'])
  })
  it('is empty when the reply is not a list or the call fails', async () => {
    h.reply = null
    expect(await installedEditorsDefaultFirst()).toEqual([])
    h.fail = true
    expect(await installedEditorsDefaultFirst()).toEqual([])
  })
})

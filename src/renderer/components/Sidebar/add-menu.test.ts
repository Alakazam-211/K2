// The nav's add menu: Create an agent beside Add Workspace, shown under the
// button that opened it.

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ADD_MENU_ADD_WORKSPACE, ADD_MENU_CREATE_AGENT, ADD_MENU_ITEMS, showAddMenu } from './add-menu'
import { useContextMenuStore } from '@/stores/context-menu'
import { resetAgentTemplatesForTests, useAgentTemplatesStore } from '@/stores/agent-templates'

const anchor = { getBoundingClientRect: () => ({ left: 10, bottom: 40 }) } as unknown as HTMLElement

beforeEach(() => {
  resetAgentTemplatesForTests()
})

describe('add menu', () => {
  it('offers Create an agent and Add Workspace', () => {
    expect(ADD_MENU_ITEMS.map((i) => i.label)).toEqual(['Create an agent', 'Add Workspace'])
  })

  it('Create an agent opens the sheet', async () => {
    const show = vi.spyOn(useContextMenuStore.getState(), 'show').mockResolvedValue(ADD_MENU_CREATE_AGENT)
    const add = vi.fn()
    await showAddMenu(anchor, add)
    expect(show).toHaveBeenCalledWith(10, 40, ADD_MENU_ITEMS)
    expect(useAgentTemplatesStore.getState().createOpen).toBe(true)
    expect(add).not.toHaveBeenCalled()
    show.mockRestore()
  })

  it('Add Workspace runs the folder pick; dismissing does nothing', async () => {
    const show = vi.spyOn(useContextMenuStore.getState(), 'show').mockResolvedValueOnce(ADD_MENU_ADD_WORKSPACE).mockResolvedValueOnce(null)
    const add = vi.fn()
    await showAddMenu(anchor, add)
    expect(add).toHaveBeenCalledTimes(1)
    await showAddMenu(anchor, add)
    expect(add).toHaveBeenCalledTimes(1)
    expect(useAgentTemplatesStore.getState().createOpen).toBe(false)
    show.mockRestore()
  })
})

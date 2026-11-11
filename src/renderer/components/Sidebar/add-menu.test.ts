// @vitest-environment jsdom
//
// The nav's add menu: Create an agent beside Add Workspace, each with its
// icon and named as the dialog it opens titles itself, shown under the
// button that opened it, or above it at the window's foot.

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ADD_MENU_ADD_WORKSPACE, ADD_MENU_CREATE_AGENT, ADD_MENU_HEIGHT, ADD_MENU_ITEMS, addMenuPosition, showAddMenu } from './add-menu'
import { useContextMenuStore } from '@/stores/context-menu'
import { CREATE_AGENT_COPY } from '@/components/AgentTemplates/CreateAgentSheet'
import { ADD_WORKSPACE_TITLE } from '@/components/AddWorkspaceDialog/AddWorkspaceDialog'
import { resetAgentTemplatesForTests, useAgentTemplatesStore } from '@/stores/agent-templates'

const anchor = { getBoundingClientRect: () => ({ left: 10, top: 16, bottom: 40 }) } as unknown as HTMLElement

beforeEach(() => {
  resetAgentTemplatesForTests()
})

describe('add menu', () => {
  it('offers Create an agent and Add Workspace, each with an icon', () => {
    expect(ADD_MENU_ITEMS.map((i) => i.label)).toEqual(['Create an agent', 'Add Workspace'])
    expect(ADD_MENU_ITEMS.every((i) => !!i.icon)).toBe(true)
  })

  it('each item is named as the dialog it opens titles itself, casing and all', () => {
    expect(ADD_MENU_ITEMS[0].label).toBe(CREATE_AGENT_COPY.title)
    expect(ADD_WORKSPACE_TITLE.startsWith(`${ADD_MENU_ITEMS[1].label} `)).toBe(true)
  })

  it('opens under the button, or above it when there is no room below', () => {
    expect(addMenuPosition({ left: 10, top: 16, bottom: 40 }, 900)).toEqual({ x: 10, y: 44 })
    // The nav footer's Add, 30px from the window's foot.
    expect(addMenuPosition({ left: 12, top: 850, bottom: 874 }, 900)).toEqual({ x: 12, y: 850 - 4 - ADD_MENU_HEIGHT })
    // Never above the window's top.
    expect(addMenuPosition({ left: 0, top: 20, bottom: 60 }, 80).y).toBe(4)
  })

  it('Create an agent opens the sheet', async () => {
    const show = vi.spyOn(useContextMenuStore.getState(), 'show').mockResolvedValue(ADD_MENU_CREATE_AGENT)
    const add = vi.fn()
    await showAddMenu(anchor, add)
    expect(show).toHaveBeenCalledWith(10, 44, ADD_MENU_ITEMS)
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

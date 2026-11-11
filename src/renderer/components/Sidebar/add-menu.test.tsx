// @vitest-environment jsdom
//
// The nav's add menu: Create an agent beside Add Workspace, each with its
// icon and named as the dialog it opens titles itself. It is the shared row
// menu (OverflowMenu): the chamfered box in Terminal, Frost in Glass,
// anchored to the caller's own button (which flips it above at the window's
// foot: useAnchoredMenu's rule, tested there).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { ADD_MENU_ADD_WORKSPACE, ADD_MENU_CREATE_AGENT, ADD_MENU_TITLE, AddMenu, addMenuItems } from './add-menu'
import { resetAgentTemplatesForTests, useAgentTemplatesStore } from '@/stores/agent-templates'
import { CREATE_AGENT_COPY } from '@/components/AgentTemplates/CreateAgentSheet'
import { ADD_WORKSPACE_TITLE } from '@/components/AddWorkspaceDialog/AddWorkspaceDialog'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

beforeEach(() => resetAgentTemplatesForTests())
afterEach(() => cleanup())

function open(addWorkspace = vi.fn()): ReturnType<typeof vi.fn> {
  render(
    <AddMenu
      addWorkspace={addWorkspace}
      trigger={(t) => (
        <button type="button" {...t} data-testid="add">
          Add
        </button>
      )}
    />,
  )
  fireEvent.click(screen.getByTestId('add'))
  return addWorkspace
}

describe('add menu', () => {
  it('offers Create an agent and Add Workspace, each with an icon', () => {
    const items = addMenuItems(() => {})
    expect(items.map((i) => i.label)).toEqual(['Create an agent', 'Add Workspace'])
    expect(items.every((i) => !!i.icon)).toBe(true)
  })

  it('each item is named as the dialog it opens titles itself, casing and all', () => {
    const [create, add] = addMenuItems(() => {})
    expect(create.label).toBe(CREATE_AGENT_COPY.title)
    expect(ADD_WORKSPACE_TITLE.startsWith(`${String(add.label)} `)).toBe(true)
  })

  it('is the shared row menu: a chamfered box, its button marked as opening it', () => {
    open()
    const trigger = screen.getByTestId('add')
    expect(trigger.getAttribute('aria-haspopup')).toBe('menu')
    expect(trigger.getAttribute('aria-expanded')).toBe('true')
    const menu = screen.getByRole('menu', { name: ADD_MENU_TITLE })
    expect(menu.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(menu.closest('[data-k2-chamfer-float="menu"]')).toBeTruthy()
    expect(menu.querySelectorAll('[data-menu-icon]')).toHaveLength(2)
  })

  it('Create an agent opens the sheet', () => {
    const add = open()
    fireEvent.click(document.querySelector(`[data-menu-item="${ADD_MENU_CREATE_AGENT}"]`)!)
    expect(useAgentTemplatesStore.getState().createOpen).toBe(true)
    expect(add).not.toHaveBeenCalled()
    expect(screen.queryByRole('menu')).toBeNull()
  })

  it('Add Workspace runs the folder pick', () => {
    const add = open()
    fireEvent.click(document.querySelector(`[data-menu-item="${ADD_MENU_ADD_WORKSPACE}"]`)!)
    expect(add).toHaveBeenCalledTimes(1)
    expect(useAgentTemplatesStore.getState().createOpen).toBe(false)
  })
})

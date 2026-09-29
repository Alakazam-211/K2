// @vitest-environment jsdom
import { useState } from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { menuLayerForTrigger, SettingDropdown } from './SettingControls'

function triggerUnder(...layers: Array<{ zIndex?: string; position?: string }>): HTMLElement {
  const root = document.createElement('div')
  let parent: HTMLElement = root
  for (const layer of layers) {
    const el = document.createElement('div')
    if (layer.position !== undefined) el.style.position = layer.position
    if (layer.zIndex !== undefined) el.style.zIndex = layer.zIndex
    parent.appendChild(el)
    parent = el
  }
  const trigger = document.createElement('button')
  parent.appendChild(trigger)
  document.body.appendChild(root)
  return trigger
}

function WorkspaceDropdown({ zIndex }: { zIndex: number }) {
  const [value, setValue] = useState('a')
  return (
    <div style={{ position: 'static', zIndex }}>
      <SettingDropdown
        ariaLabel="Workspace"
        value={value}
        onChange={setValue}
        options={[
          { value: 'a', label: 'Alpha' },
          { value: 'b', label: 'Beta' },
        ]}
      />
    </div>
  )
}

afterEach(() => {
  cleanup()
  document.body.replaceChildren()
})

describe('menuLayerForTrigger', () => {
  it('stays at 400 when the highest ancestor is 200', () => {
    const trigger = triggerUnder({ zIndex: '200', position: 'static' })
    expect(menuLayerForTrigger(trigger)).toBe(400)
  })

  it('stays at 400 when no ancestor has a numeric z-index', () => {
    const trigger = triggerUnder()
    expect(menuLayerForTrigger(trigger)).toBe(400)
  })

  it('stays at 400 when 50 is inside 200', () => {
    const trigger = triggerUnder(
      { zIndex: '200', position: 'static' },
      { zIndex: '50', position: 'static' },
    )
    expect(menuLayerForTrigger(trigger)).toBe(400)
  })

  it('opens at 1001 above an ancestor of 1000', () => {
    const trigger = triggerUnder({ zIndex: '1000', position: 'static' })
    expect(menuLayerForTrigger(trigger)).toBe(1001)
  })

  it('opens at 100000 above a static ancestor of 99999', () => {
    const trigger = triggerUnder({ zIndex: '99999', position: 'static' })
    expect(getComputedStyle(trigger.parentElement!).position).toBe('static')
    expect(getComputedStyle(trigger.parentElement!).zIndex).toBe('99999')
    expect(menuLayerForTrigger(trigger)).toBe(100000)
  })

  it('ignores an ancestor whose z-index is auto', () => {
    const onlyAuto = triggerUnder({ zIndex: 'auto', position: 'static' })
    expect(menuLayerForTrigger(onlyAuto)).toBe(400)

    const autoInside = triggerUnder(
      { zIndex: '1000', position: 'static' },
      { zIndex: 'auto', position: 'static' },
    )
    expect(getComputedStyle(autoInside.parentElement!).zIndex).toBe('auto')
    expect(menuLayerForTrigger(autoInside)).toBe(1001)
  })
})

describe('SettingDropdown menu layer', () => {
  it('opens at 400 on document.body under a parent z-index of 200', () => {
    render(<WorkspaceDropdown zIndex={200} />)
    fireEvent.click(screen.getByRole('button', { name: 'Workspace' }))
    const menu = screen.getByTestId('setting-dropdown-menu')
    expect(menu.parentElement).toBe(document.body)
    expect(menu.style.zIndex).toBe('400')
    expect(menu.className).toContain('bg-[var(--color-bg)]')
    expect(menu.className).not.toContain('--color-bg-surface')
    expect(menu.className).not.toContain('--color-bg-elevated')
    expect(menu.getAttribute('style') ?? '').not.toContain('z-index: 9999')
  })
})

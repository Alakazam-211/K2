// @vitest-environment jsdom
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { menuLayerForTrigger, SaveError, SaveNote, SETTING_ROW_HINT, SettingDropdown, SettingRow, SettingsGroup, useSaveStatus } from './SettingControls'

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
    const box = menu.firstElementChild as HTMLElement
    expect(box.className).toContain('bg-[var(--color-bg)]')
    expect(box.className).not.toContain('--color-bg-surface')
    expect(box.className).not.toContain('--color-bg-elevated')
    expect(menu.getAttribute('style') ?? '').not.toContain('z-index: 9999')
  })

  it('is frosted floating chrome in Glass, and keeps the table\'s material in a Garden', () => {
    render(<WorkspaceDropdown zIndex={0} />)
    fireEvent.click(screen.getByRole('button', { name: 'Workspace' }))
    const box = screen.getByTestId('setting-dropdown-menu').firstElementChild as HTMLElement
    expect(box.getAttribute('data-material')).toBe('frost')
    expect(box.getAttribute('data-float')).toBe('menu')
    cleanup()
    render(<div data-zen-root=""><WorkspaceDropdown zIndex={0} /></div>)
    fireEvent.click(screen.getByRole('button', { name: 'Workspace' }))
    const garden = screen.getByTestId('setting-dropdown-menu').firstElementChild as HTMLElement
    expect(garden.getAttribute('data-material')).not.toBe('frost')
  })
})

describe('Hints', () => {
  it('wrap at a reading measure, in rows and under a group title', () => {
    render(
      <SettingsGroup title="Group" hint="Group hint" card>
        <SettingRow label="Row" hint="Row hint"><span /></SettingRow>
      </SettingsGroup>,
    )
    expect(screen.getByText('Group hint').className).toContain('max-w-[75ch]')
    expect(screen.getByText('Row hint').className).toContain('max-w-[75ch]')
    expect(SETTING_ROW_HINT).toContain('max-w-[75ch]')
  })
})

describe('SettingDropdown Esc (0.46.3)', () => {
  it('Esc closes only the open menu: the Settings panel listener never sees it', () => {
    // Stands in for Settings.tsx: a bubble-phase window listener that
    // closes the panel unless the Esc was claimed (defaultPrevented).
    let panelClosed = 0
    const settingsEsc = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !e.defaultPrevented) panelClosed += 1
    }
    window.addEventListener('keydown', settingsEsc)
    try {
      render(<WorkspaceDropdown zIndex={200} />)
      fireEvent.click(screen.getByRole('button', { name: 'Workspace' }))
      expect(screen.getByTestId('setting-dropdown-menu')).toBeTruthy()
      fireEvent.keyDown(screen.getByTestId('setting-dropdown-menu'), { key: 'Escape' })
      expect(screen.queryByTestId('setting-dropdown-menu')).toBeNull()
      expect(panelClosed).toBe(0)
      // With the menu closed, Esc is the panel's again.
      fireEvent.keyDown(document.body, { key: 'Escape' })
      expect(panelClosed).toBe(1)
    } finally {
      window.removeEventListener('keydown', settingsEsc)
    }
  })
})

function SaveHarness({ fail }: { fail?: unknown }): React.JSX.Element {
  const save = useSaveStatus()
  return (
    <>
      <button type="button" onClick={() => (fail === undefined ? save.saved() : save.failed(fail))}>Write</button>
      <SaveNote state={save.state} />
      <SaveError state={save.state} />
    </>
  )
}

describe('Save confirmation', () => {
  it('shows "Saved" by the control for two seconds after a write lands', () => {
    vi.useFakeTimers()
    try {
      render(<SaveHarness />)
      expect(screen.queryByRole('status')).toBeNull()
      fireEvent.click(screen.getByRole('button', { name: 'Write' }))
      expect(screen.getByRole('status').textContent).toBe('Saved')
      act(() => { vi.advanceTimersByTime(1999) })
      expect(screen.getByRole('status')).toBeTruthy()
      act(() => { vi.advanceTimersByTime(1) })
      expect(screen.queryByRole('status')).toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })

  it('a failed write says why, that nothing changed, and to try again', () => {
    render(<SaveHarness fail={new Error('the server refused.')} />)
    fireEvent.click(screen.getByRole('button', { name: 'Write' }))
    expect(screen.getByRole('alert').textContent).toBe('Couldn\u2019t save: the server refused. Nothing changed; try again.')
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('a failure with no message still says something', () => {
    render(<SaveHarness fail={{}} />)
    fireEvent.click(screen.getByRole('button', { name: 'Write' }))
    expect(screen.getByRole('alert').textContent).toContain('K2 couldn\u2019t reach this server')
  })
})

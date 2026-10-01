// @vitest-environment jsdom
//
// prd-remove-viewer-role-v1.md T3 — Settings → Server Access → Users:
// the role picker has no Viewer option, and a former Viewer row offers
// "Enable as Member". Fail loud: no defaults in assertions.

import { describe, it, expect, vi, afterEach } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'

import { SettingDropdown } from '../controls/SettingControls'
import {
  ROLE_OPTIONS,
  WAS_VIEWER_NOTE,
  needsEnableAsMember,
  parseK2Role,
} from './connect-user-roles'
import { WasViewerBadge, WasViewerNotice } from './WasViewerNotice'

afterEach(() => cleanup())

describe('role picker — no Viewer option', () => {
  it('offers exactly member, admin, owner', () => {
    expect(ROLE_OPTIONS.map((o) => o.value)).toEqual(['member', 'admin', 'owner'])
    expect(ROLE_OPTIONS.map((o) => o.label)).toEqual(['Member', 'Admin', 'Owner'])
  })

  it('the rendered dropdown menu has no Viewer entry', () => {
    render(
      <SettingDropdown
        value="member"
        options={[...ROLE_OPTIONS]}
        ariaLabel="New user role"
        onChange={() => {}}
      />,
    )
    fireEvent.click(screen.getByLabelText('New user role'))
    const menu = screen.getByTestId('setting-dropdown-menu')
    const names = [...menu.querySelectorAll('button')].map((b) => b.textContent)
    expect(names).toEqual(['Member', 'Admin', 'Owner'])
    expect(names.some((n) => n !== null && /viewer/i.test(n))).toBe(false)
  })

  it('an older server still reporting viewer shows it as the placeholder, not a choice', () => {
    render(
      <SettingDropdown
        value="viewer"
        placeholder="viewer"
        options={[...ROLE_OPTIONS]}
        ariaLabel="vera role"
        onChange={() => {}}
      />,
    )
    const trigger = screen.getByLabelText('vera role')
    expect(trigger.textContent).toContain('viewer')
    fireEvent.click(trigger)
    const menu = screen.getByTestId('setting-dropdown-menu')
    const names = [...menu.querySelectorAll('button')].map((b) => b.textContent)
    expect(names).toEqual(['Member', 'Admin', 'Owner'])
  })

  it('parseK2Role refuses viewer and unknown strings instead of coercing', () => {
    expect(parseK2Role('owner')).toBe('owner')
    expect(parseK2Role('admin')).toBe('admin')
    expect(parseK2Role('member')).toBe('member')
    expect(parseK2Role('viewer')).toBeNull()
    expect(parseK2Role('Viewer')).toBeNull()
    expect(parseK2Role(undefined)).toBeNull()
    expect(parseK2Role(42)).toBeNull()
  })
})

describe('was Viewer → Enable as Member', () => {
  it('needsEnableAsMember only for a disabled former viewer', () => {
    expect(needsEnableAsMember({ wasViewer: true, disabled: true })).toBe(true)
    expect(needsEnableAsMember({ wasViewer: true, disabled: false })).toBe(false)
    expect(needsEnableAsMember({ wasViewer: false, disabled: true })).toBe(false)
    expect(needsEnableAsMember({ disabled: true })).toBe(false)
  })

  it('renders the note and calls back with the username on click', () => {
    const onEnable = vi.fn()
    render(<WasViewerNotice username="vera" busy={false} onEnableAsMember={onEnable} />)
    expect(screen.getByTestId('was-viewer-vera').textContent).toContain(WAS_VIEWER_NOTE)
    const button = screen.getByRole('button', { name: 'Enable vera as Member' })
    expect(button.textContent).toBe('Enable as Member')
    fireEvent.click(button)
    expect(onEnable).toHaveBeenCalledTimes(1)
    expect(onEnable).toHaveBeenCalledWith('vera')
  })

  it('is disabled and relabelled while the enable is in flight', () => {
    const onEnable = vi.fn()
    render(<WasViewerNotice username="vera" busy={true} onEnableAsMember={onEnable} />)
    const button = screen.getByRole('button', { name: 'Enable vera as Member' }) as HTMLButtonElement
    expect(button.disabled).toBe(true)
    expect(button.textContent).toBe('Enabling…')
    fireEvent.click(button)
    expect(onEnable).not.toHaveBeenCalled()
  })

  it('the badge says was Viewer', () => {
    render(<WasViewerBadge />)
    expect(screen.getByText('was Viewer')).not.toBeNull()
  })
})

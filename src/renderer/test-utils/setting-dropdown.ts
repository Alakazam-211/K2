// Test helpers for K2's themed menu (SettingDropdown). The menu portals to
// <body>, so these read it from the document, not from the trigger.
//
// The sync helpers must not run inside `waitFor`: React does not flush the
// menu's layout effect there, so the menu never shows. For options that load
// after render, use `waitForMenuOptions`.

import { fireEvent, screen, waitFor } from '@testing-library/react'

export interface MenuOption {
  value: string
  label: string
  disabled: boolean
}

function menuButtons(): HTMLButtonElement[] {
  const menu = screen.queryByTestId('setting-dropdown-menu')
  if (!menu) throw new Error('the K2 menu is not open (inside waitFor? use waitForMenuOptions)')
  return [...menu.querySelectorAll<HTMLButtonElement>('button[data-value]')]
}

function readMenu(): MenuOption[] {
  return menuButtons().map((b) => ({
    value: b.getAttribute('data-value') ?? '',
    label: b.textContent ?? '',
    disabled: b.disabled,
  }))
}

function asTrigger(el: HTMLElement): HTMLButtonElement {
  if (el.tagName !== 'BUTTON') throw new Error(`expected a SettingDropdown trigger button, got <${el.tagName.toLowerCase()}>`)
  return el as HTMLButtonElement
}

function close(t: HTMLButtonElement): void {
  fireEvent.click(t)
  if (screen.queryByTestId('setting-dropdown-menu')) throw new Error('the K2 menu did not close')
}

/** Open the menu, read its options, then close it again. */
export function menuOptions(trigger: HTMLElement): MenuOption[] {
  const t = asTrigger(trigger)
  fireEvent.click(t)
  const out = readMenu()
  close(t)
  return out
}

export const menuValues = (trigger: HTMLElement): string[] => menuOptions(trigger).map((o) => o.value)
export const menuLabels = (trigger: HTMLElement): string[] => menuOptions(trigger).map((o) => o.label)

/** Open the menu, wait until its options pass `check` (they may load after
 *  render), then close it. */
export async function waitForMenuOptions(trigger: HTMLElement, check: (options: MenuOption[]) => void): Promise<void> {
  const t = asTrigger(trigger)
  fireEvent.click(t)
  await screen.findByTestId('setting-dropdown-menu')
  await waitFor(() => check(readMenu()))
  close(t)
}

/** The value of the option the menu marks as picked, or null. Opens and
 *  closes the menu. */
export function pickedValue(trigger: HTMLElement): string | null {
  const t = asTrigger(trigger)
  fireEvent.click(t)
  const active = menuButtons().find((b) => b.hasAttribute('data-selected'))
  close(t)
  return active ? active.getAttribute('data-value') : null
}

/** Open the menu and pick the option with this value the way a person does
 *  (mousedown, then click), so a host dialog sees the mousedown too. */
export function pickValue(trigger: HTMLElement, value: string): void {
  fireEvent.click(asTrigger(trigger))
  const opts = menuButtons()
  const b = opts.find((o) => o.getAttribute('data-value') === value)
  if (!b) throw new Error(`no option with value "${value}"; have ${JSON.stringify(opts.map((o) => o.getAttribute('data-value')))}`)
  fireEvent.mouseDown(b)
  fireEvent.click(b)
}

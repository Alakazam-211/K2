// The top bar's right capsule as Glass draws it: frosted glass, whose
// backdrop-filter makes it a stacking context and a containing block. A
// menu or dialog left inside it is placed against the capsule and painted
// under the tiles, so every one it opens must render on the app's popover
// layer at the top of <body> instead.
import { expect } from 'vitest'

/** Wrap a top-bar control the way the top bar holds it. */
export function FrostCapsule({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div data-material="frost" data-frost-shape="pill" data-top-bar-cluster="">
      {children}
    </div>
  )
}

/** An anchored menu: out of the capsule, fixed, on the menu layer. */
export function expectOnPopoverLayer(menu: HTMLElement): void {
  expect(menu.closest('[data-top-bar-cluster]'), 'inside the frosted capsule').toBeNull()
  expect(menu.parentElement).toBe(document.body)
  expect(menu.style.position).toBe('fixed')
  expect(Number(menu.style.zIndex)).toBeGreaterThanOrEqual(400)
}

/** A modal: out of the capsule, at the top of <body>. */
export function expectModalOnTopLayer(dialog: HTMLElement): void {
  expect(dialog.closest('[data-top-bar-cluster]'), 'inside the frosted capsule').toBeNull()
  expect(dialog.parentElement).toBe(document.body)
}

// @vitest-environment jsdom
// K2 Companion `c29b6db` (Rosson 2026-10-18), on every desktop message box
// with the pieces (the desktop Thread, Zen and Garden composers):
//   - the "+" is lit while pressed (pointerdown until up / cancel / leave)
//     and while its menu is open: a soft glass fill (the text colour at
//     20%, white at 20% on dark) and the glyph at full ink, 90ms;
//   - Send brightens (`brightness(1.5)`, 90ms ease-out) while pressed, and
//     only while it can send.
import { afterEach, describe, expect, it } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { ZenCompose } from '@/components/Zen/widgets/ZenCompose'
import {
  THREAD_PLUS_LIT_FILL,
  THREAD_PLUS_LIT_INK,
  THREAD_SEND_PRESSED_FILTER,
  ThreadRoundButton,
  ThreadSendButton,
} from './ThreadZenKit'

afterEach(cleanup)

function el(sel: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(sel)
  if (!found) throw new Error(`missing ${sel}`)
  return found
}

function expectLit(b: HTMLElement): void {
  expect(b.getAttribute('data-zen-lit')).toBe('true')
  expect(b.style.background).toBe(THREAD_PLUS_LIT_FILL)
  expect(b.style.color).toBe(THREAD_PLUS_LIT_INK)
}

function expectAtRest(b: HTMLElement): void {
  expect(b.hasAttribute('data-zen-lit')).toBe(false)
  expect(b.style.background).toBe('')
  expect(b.style.color).toBe('var(--zen-text-muted)')
}

describe('the lit "+"', () => {
  it('the tokens: the text colour at 20% and at full ink (white glass on dark, no white on white)', () => {
    expect(THREAD_PLUS_LIT_FILL).toBe('color-mix(in srgb, var(--zen-text) 20%, transparent)')
    expect(THREAD_PLUS_LIT_INK).toBe('var(--zen-text)')
  })

  it('lit from pointerdown until pointerup, pointercancel or pointerleave, with a 90ms fill', () => {
    render(
      <ThreadRoundButton label="Attach files" lightable onClick={() => {}}>
        +
      </ThreadRoundButton>,
    )
    const b = el('button[aria-label="Attach files"]')
    expectAtRest(b)
    expect(b.style.transition).toContain('background-color 90ms ease-out')
    expect(b.style.transition).toContain('color 90ms ease-out')
    for (const end of ['pointerUp', 'pointerCancel', 'pointerLeave'] as const) {
      fireEvent.pointerDown(b)
      expectLit(b)
      fireEvent[end](b)
      expectAtRest(b)
    }
  })

  it('lit while its menu is open, pressed or not', () => {
    const { rerender } = render(
      <ThreadRoundButton label="Add" lightable menuOpen onClick={() => {}}>
        +
      </ThreadRoundButton>,
    )
    const b = el('button[aria-label="Add"]')
    expectLit(b)
    fireEvent.pointerDown(b)
    fireEvent.pointerUp(b)
    expectLit(b)
    rerender(
      <ThreadRoundButton label="Add" lightable menuOpen={false} onClick={() => {}}>
        +
      </ThreadRoundButton>,
    )
    expectAtRest(b)
  })

  it('never lit while disabled, and not at all unless lightable', () => {
    render(
      <>
        <ThreadRoundButton label="Off" lightable disabled menuOpen onClick={() => {}}>
          +
        </ThreadRoundButton>
        <ThreadRoundButton label="Slash" onClick={() => {}}>
          /
        </ThreadRoundButton>
      </>,
    )
    const off = el('button[aria-label="Off"]')
    fireEvent.pointerDown(off)
    expectAtRest(off)
    const slash = el('button[aria-label="Slash"]')
    fireEvent.pointerDown(slash)
    expectAtRest(slash)
    expect(slash.style.transition).toBe('')
  })

  it('the Zen / Garden composer’s "+" lights while pressed', () => {
    render(<ZenCompose bridge={{} as ZenWidgetBridge} address="press-test-plus" label="Ada" disabled={false} />)
    const plus = el('[data-zen-attach]')
    expectAtRest(plus)
    fireEvent.pointerDown(plus)
    expectLit(plus)
    fireEvent.pointerLeave(plus)
    expectAtRest(plus)
  })
})

describe('Send brightens while pressed', () => {
  it('brightness(1.5) from pointerdown to pointerup, with a 90ms ease-out', () => {
    render(<ThreadSendButton canSend onClick={() => {}} />)
    const send = el('[data-zen-send]')
    expect(send.style.filter).toBe('')
    expect(send.hasAttribute('data-zen-pressed')).toBe(false)
    expect(send.style.transition).toContain('filter 90ms ease-out')
    for (const end of ['pointerUp', 'pointerCancel', 'pointerLeave'] as const) {
      fireEvent.pointerDown(send)
      expect(send.style.filter).toBe(THREAD_SEND_PRESSED_FILTER)
      expect(THREAD_SEND_PRESSED_FILTER).toBe('brightness(1.5)')
      expect(send.getAttribute('data-zen-pressed')).toBe('true')
      fireEvent[end](send)
      expect(send.style.filter).toBe('')
      expect(send.hasAttribute('data-zen-pressed')).toBe(false)
    }
  })

  it('not when it can’t send, and it lets go when sending stops being possible', () => {
    const { rerender } = render(<ThreadSendButton canSend={false} onClick={() => {}} />)
    const send = el('[data-zen-send]')
    fireEvent.pointerDown(send)
    expect(send.style.filter).toBe('')
    expect(send.hasAttribute('data-zen-pressed')).toBe(false)
    fireEvent.pointerUp(send)
    rerender(<ThreadSendButton canSend onClick={() => {}} />)
    fireEvent.pointerDown(send)
    expect(send.style.filter).toBe(THREAD_SEND_PRESSED_FILTER)
    rerender(<ThreadSendButton canSend={false} onClick={() => {}} />)
    expect(send.style.filter).toBe('')
    expect(send.hasAttribute('data-zen-pressed')).toBe(false)
  })

  it('the Zen / Garden composer’s Send: only once there is something to send', async () => {
    render(<ZenCompose bridge={{} as ZenWidgetBridge} address="press-test-send" label="Ada" disabled={false} />)
    const send = el('[data-zen-send]')
    fireEvent.pointerDown(send)
    expect(send.style.filter).toBe('')
    fireEvent.pointerUp(send)
    await act(async () => {
      fireEvent.change(el('[data-zen-compose-input]'), { target: { value: 'hello' } })
    })
    fireEvent.pointerDown(send)
    expect(send.style.filter).toBe(THREAD_SEND_PRESSED_FILTER)
    fireEvent.pointerUp(send)
    expect(send.style.filter).toBe('')
  })
})

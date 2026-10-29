// @vitest-environment jsdom
// The Mobile tab's loading screen (Rosson 2026-10-29), fail loud:
//  - the steps follow the session state: installing (first use only) →
//    starting the device → connecting the screen; passed steps are done;
//  - an elapsed counter ticks, and the first-start hint shows after 10 s;
//  - a start that stopped shows the error, a failed step and Try again.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { BOOT_HINT, formatElapsed, MobileBootScreen } from './MobileBootScreen'

function steps(): Array<[string, string]> {
  return Array.from(document.querySelectorAll('[data-mobile-boot-step]')).map((el) => [
    el.getAttribute('data-mobile-boot-step') ?? '',
    el.getAttribute('data-state') ?? '',
  ])
}

function text(sel: string): string | null {
  return document.querySelector(sel)?.textContent ?? null
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-10-29T12:00:00Z'))
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('MobileBootScreen', () => {
  it('formats elapsed time as m:ss', () => {
    expect(formatElapsed(0)).toBe('0:00')
    expect(formatElapsed(9_999)).toBe('0:09')
    expect(formatElapsed(65_000)).toBe('1:05')
  })

  it('steps follow the phase; the install step shows only on a first use', () => {
    const t0 = Date.now()
    const { rerender } = render(<MobileBootScreen deviceName="iPhone 16" phase="booting" startedAt={t0} withInstall={false} />)
    expect(text('[data-mobile-boot-title]')).toBe('Starting iPhone 16')
    expect(steps()).toEqual([
      ['booting', 'now'],
      ['connecting', 'next'],
    ])
    rerender(<MobileBootScreen deviceName="iPhone 16" phase="connecting" startedAt={t0} withInstall={false} />)
    expect(steps()).toEqual([
      ['booting', 'done'],
      ['connecting', 'now'],
    ])
    rerender(<MobileBootScreen deviceName="iPhone 16" phase="installing" startedAt={t0} withInstall={false} />)
    expect(steps()).toEqual([
      ['installing', 'now'],
      ['booting', 'next'],
      ['connecting', 'next'],
    ])
    rerender(<MobileBootScreen deviceName="iPhone 16" phase="booting" startedAt={t0} withInstall />)
    expect(steps()).toEqual([
      ['installing', 'done'],
      ['booting', 'now'],
      ['connecting', 'next'],
    ])
    expect(text('[data-mobile-boot-step="booting"]')).toBe('Starting iPhone 16')
  })

  it('the elapsed counter ticks and the first-start hint shows after 10 s', () => {
    render(<MobileBootScreen deviceName="Pixel 9" phase="booting" startedAt={Date.now()} withInstall={false} />)
    expect(text('[data-mobile-boot-elapsed]')).toBe('0:00')
    expect(document.querySelector('[data-mobile-boot-hint]')).toBeNull()
    act(() => {
      vi.advanceTimersByTime(9_000)
    })
    expect(text('[data-mobile-boot-elapsed]')).toBe('0:09')
    expect(document.querySelector('[data-mobile-boot-hint]')).toBeNull()
    act(() => {
      vi.advanceTimersByTime(1_000)
    })
    expect(text('[data-mobile-boot-elapsed]')).toBe('0:10')
    expect(text('[data-mobile-boot-hint]')).toBe(BOOT_HINT)
  })

  it('an error marks the step failed, shows the text and Try again', () => {
    const onRetry = vi.fn()
    render(
      <MobileBootScreen
        deviceName="iPhone 16"
        phase="booting"
        startedAt={Date.now() - 30_000}
        withInstall={false}
        error="The device stopped: simctl boot failed"
        onRetry={onRetry}
      />,
    )
    expect(document.querySelector('[data-mobile-boot]')?.getAttribute('data-mobile-boot')).toBe('failed')
    expect(text('[data-mobile-boot-title]')).toBe("iPhone 16 didn't start")
    expect(steps()).toEqual([
      ['booting', 'failed'],
      ['connecting', 'next'],
    ])
    expect(text('[data-mobile-boot-error]')).toBe('The device stopped: simctl boot failed')
    expect(document.querySelector('[data-mobile-boot-hint]')).toBeNull()
    expect(document.querySelector('[data-mobile-boot-elapsed]')).toBeNull()
    fireEvent.click(document.querySelector('[data-mobile-boot-retry]') as HTMLButtonElement)
    expect(onRetry).toHaveBeenCalledTimes(1)
  })

  it('an error with no retry (watch-only, stream closed) has no button', () => {
    render(
      <MobileBootScreen deviceName="Pixel 9" phase="connecting" startedAt={Date.now()} withInstall={false} error="The device stream closed." />,
    )
    expect(text('[data-mobile-boot-error]')).toBe('The device stream closed.')
    expect(document.querySelector('[data-mobile-boot-retry]')).toBeNull()
  })
})

import { describe, expect, it } from 'vitest'
import {
  SETI_FONT_RETRY_CAP_MS,
  SETI_FONT_STYLE_ID,
  createSetiFontLoader,
  setiFontRetryDelay,
  type SetiFontEnv,
} from './seti-font'

interface FakeStyle {
  id: string
  textContent: string
}

/** Minimal document: one <head>, getElementById over appended styles, and a
 *  `fonts.load` that answers from a scripted list of outcomes. */
function fakeEnv(outcomes: Array<'ok' | 'fail' | 'empty'>) {
  const styles: FakeStyle[] = []
  const loads: string[] = []
  const timers: Array<{ fn: () => void; ms: number }> = []
  const doc = {
    head: {
      appendChild: (el: FakeStyle) => {
        styles.push(el)
        return el
      },
    },
    getElementById: (id: string) => styles.find((s) => s.id === id) ?? null,
    createElement: () => ({ id: '', textContent: '' }),
    fonts: {
      load: (font: string) => {
        // Record the rule text each load sees: a retry must see NEW text.
        loads.push(`${font}|${styles[0]?.textContent ?? ''}`)
        const next = outcomes.shift()
        if (next === undefined) throw new Error('unexpected extra fonts.load')
        if (next === 'fail') return Promise.reject(new Error('NetworkError'))
        return Promise.resolve(next === 'ok' ? [{}] : [])
      },
    },
  }
  const env = {
    document: doc as unknown as SetiFontEnv['document'],
    setTimeout: (fn: () => void, ms: number) => {
      timers.push({ fn, ms })
      return timers.length
    },
  }
  return { env, styles, loads, timers }
}

const flush = () => new Promise((r) => setTimeout(r, 0))

describe('seti font loader', () => {
  it('loads once on success and never re-injects', async () => {
    const f = fakeEnv(['ok'])
    const l = createSetiFontLoader('/assets/seti-X.woff', f.env)
    l.ensure()
    l.ensure()
    await flush()
    l.ensure()
    expect(l.state).toBe('ready')
    expect(f.loads).toHaveLength(1)
    expect(f.styles).toHaveLength(1)
    expect(f.styles[0].id).toBe(SETI_FONT_STYLE_ID)
    expect(f.styles[0].textContent).toContain("url('/assets/seti-X.woff')")
    expect(f.timers).toHaveLength(0)
  })

  it('a failed load is retried with a fresh @font-face rule, not left as boxes', async () => {
    const f = fakeEnv(['fail', 'empty', 'ok'])
    const l = createSetiFontLoader('/assets/seti-X.woff', f.env)
    l.ensure()
    await flush()
    expect(l.state).toBe('waiting')
    expect(l.failures).toBe(1)
    // Renders while waiting do not stack loads.
    l.ensure()
    expect(f.loads).toHaveLength(1)
    expect(f.timers.map((t) => t.ms)).toEqual([1000])

    f.timers[0].fn()
    await flush()
    expect(l.state).toBe('waiting')
    expect(l.failures).toBe(2)
    expect(f.timers.map((t) => t.ms)).toEqual([1000, 2000])

    f.timers[1].fn()
    await flush()
    expect(l.state).toBe('ready')
    expect(l.failures).toBe(0)
    // One style element, rewritten each time: every load saw different text.
    expect(f.styles).toHaveLength(1)
    expect(f.loads).toHaveLength(3)
    expect(new Set(f.loads).size).toBe(3)
  })

  it('backoff doubles and caps', () => {
    expect([1, 2, 3, 4, 5, 6, 7].map(setiFontRetryDelay)).toEqual([
      1000, 2000, 4000, 8000, 16000, SETI_FONT_RETRY_CAP_MS, SETI_FONT_RETRY_CAP_MS,
    ])
  })
})

// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { DOC_HIDDEN_ATTR, installDocHiddenAttr } from './doc-hidden'

let state: DocumentVisibilityState = 'visible'
Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state })

function flip(next: DocumentVisibilityState): void {
  state = next
  document.dispatchEvent(new Event('visibilitychange'))
}

let uninstall: (() => void) | null = null
afterEach(() => {
  uninstall?.()
  uninstall = null
  state = 'visible'
  document.documentElement.removeAttribute(DOC_HIDDEN_ATTR)
})

describe('installDocHiddenAttr', () => {
  it('flips data-doc-hidden on <html> with the page’s visibility', () => {
    uninstall = installDocHiddenAttr()
    expect(document.documentElement.hasAttribute(DOC_HIDDEN_ATTR)).toBe(false)
    flip('hidden')
    expect(document.documentElement.hasAttribute(DOC_HIDDEN_ATTR)).toBe(true)
    flip('visible')
    expect(document.documentElement.hasAttribute(DOC_HIDDEN_ATTR)).toBe(false)
  })

  it('reads the state at install (a window that starts hidden)', () => {
    state = 'hidden'
    uninstall = installDocHiddenAttr()
    expect(document.documentElement.hasAttribute(DOC_HIDDEN_ATTR)).toBe(true)
  })

  it('stops listening once uninstalled', () => {
    installDocHiddenAttr()()
    flip('hidden')
    expect(document.documentElement.hasAttribute(DOC_HIDDEN_ATTR)).toBe(false)
  })

  it('is installed at startup', () => {
    const dir = dirname(fileURLToPath(import.meta.url))
    const index = readFileSync(join(dir, '../index.tsx'), 'utf8')
    expect(index).toMatch(/^installDocHiddenAttr\(\)$/m)
  })
})

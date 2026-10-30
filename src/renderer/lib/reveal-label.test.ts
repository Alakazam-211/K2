// @vitest-environment jsdom
// File-manager words by desktop: Finder / Trash on a Mac, File Explorer /
// Recycle Bin on Windows, Files / Trash on Linux.

import { afterEach, describe, expect, it } from 'vitest'
import { revealInFileManagerLabel, trashName } from './reveal-label'

const realUa = navigator.userAgent
const realPlatform = navigator.platform

function setNav(platform: string, userAgent: string): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  Object.defineProperty(window.navigator, 'userAgent', { value: userAgent, configurable: true })
}

afterEach(() => setNav(realPlatform, realUa))

describe('file-manager words', () => {
  it('Windows: Open in Explorer, Recycle Bin', () => {
    setNav('Win32', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/131.0')
    expect(revealInFileManagerLabel()).toBe('Open in Explorer')
    expect(trashName()).toBe('Recycle Bin')
  })

  it('macOS: Open in Finder, Trash', () => {
    setNav('MacIntel', 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15')
    expect(revealInFileManagerLabel()).toBe('Open in Finder')
    expect(trashName()).toBe('Trash')
  })

  it('Linux: Show in Files, Trash', () => {
    setNav('Linux x86_64', 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15')
    expect(revealInFileManagerLabel()).toBe('Show in Files')
    expect(trashName()).toBe('Trash')
  })
})

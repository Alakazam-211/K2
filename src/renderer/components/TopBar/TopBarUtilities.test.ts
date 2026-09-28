import { createElement } from 'react'
import { describe, expect, it } from 'vitest'
import { hasTopBarPageToggles } from './TopBarUtilities'

describe('hasTopBarPageToggles', () => {
  it('is false when the page hides the drawer controls', () => {
    expect(hasTopBarPageToggles(undefined)).toBe(false)
    expect(hasTopBarPageToggles(null)).toBe(false)
    expect(hasTopBarPageToggles(false)).toBe(false)
  })

  it('is true when a drawer control is passed', () => {
    expect(hasTopBarPageToggles(createElement('button'))).toBe(true)
  })
})

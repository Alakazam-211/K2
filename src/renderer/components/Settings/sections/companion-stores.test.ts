import { describe, expect, it } from 'vitest'
import { APP_STORE_URL, PLAY_STORE_URL } from './CompanionSection'

describe('K2 Companion store listings (k2.dev home badges)', () => {
  it('App Store URL matches k2.dev', () => {
    expect(APP_STORE_URL).toBe('https://apps.apple.com/us/app/k2so/id6762076766')
  })

  it('Google Play URL matches k2.dev', () => {
    expect(PLAY_STORE_URL).toBe(
      'https://play.google.com/store/apps/details?id=com.alakazamlabs.k2so.companion',
    )
  })
})

// @vitest-environment jsdom
// Settings → 3rd Party Bots → Hermes / Instinct / Muse / Dots (0.45.7):
// placeholder pages and the SOON tag in the nav.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { COMING_SOON_BOTS, ComingSoonBotSection, comingSoonBotCopy } from './ComingSoonBotSection'

afterEach(() => cleanup())

describe('coming-soon bot pages', () => {
  it('each page names the provider and says what K2 will do', () => {
    expect(COMING_SOON_BOTS.map((b) => b.name)).toEqual(['Hermes', 'Instinct', 'Muse', 'Dots'])
    for (const bot of COMING_SOON_BOTS) {
      render(<ComingSoonBotSection id={bot.id} />)
      const page = document.querySelector(`[data-coming-soon-bot="${bot.id}"]`) as HTMLElement
      expect(screen.getByRole('heading', { name: bot.name })).toBeTruthy()
      expect(page.querySelector('p')?.textContent).toBe(
        `${bot.name} support is coming soon — K2 will detect ${bot.name} agents on this server and let you use them like Grok Bot.`,
      )
      expect(comingSoonBotCopy(bot.name)).toBe(page.querySelector('p')?.textContent)
      cleanup()
    }
  })

  it('renders nothing for an id that is not a coming-soon bot', () => {
    const { container } = render(<ComingSoonBotSection id="local-grokbots" />)
    expect(container.innerHTML).toBe('')
  })

  it('Settings routes the pages and the nav tags those leaves SOON (clickable, like BETA)', () => {
    const src = readFileSync(join(process.cwd(), 'src/renderer/components/Settings/Settings.tsx'), 'utf8')
    expect(src).toContain('<ComingSoonBotSection id={activeSection} />')
    expect(src).toContain('comingSoon={it.comingSoon}')
    const button = src.slice(src.indexOf('function SettingsNavButton'))
    expect(button).toContain('>SOON<')
    expect(src).toContain('...COMING_SOON_BOTS_MANIFEST,')
  })
})

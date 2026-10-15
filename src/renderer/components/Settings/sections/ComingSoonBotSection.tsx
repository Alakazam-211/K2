// Settings → 3rd Party Bots → Hermes / Instinct / Muse / Dots (0.45.7,
// Rosson). Placeholder pages for bot providers K2 doesn't support yet; the
// nav marks each leaf "Soon". When a provider ships, its row in
// `THIRD_PARTY_BOTS` (Settings.tsx) gets a capability and a real page, like
// Grok Bot.

import React from 'react'
import type { SettingEntry } from '../searchManifest'
import type { SettingsSection } from '@/stores/settings'

/** The providers that are coming, in nav order, with their CLI prefixes. */
export const COMING_SOON_BOTS: { id: SettingsSection; name: string; cliPrefix: string }[] = [
  { id: 'bots-hermes', name: 'Hermes', cliPrefix: 'hm' },
  { id: 'bots-instinct', name: 'Instinct', cliPrefix: 'in' },
  { id: 'bots-muse', name: 'Muse', cliPrefix: 'ms' },
  { id: 'bots-dots', name: 'Dots', cliPrefix: 'dt' },
]

export function comingSoonBotCopy(name: string): string {
  return `${name} support is coming soon — K2 will detect ${name} agents on this server and let you use them like Grok Bot.`
}

export const COMING_SOON_BOTS_MANIFEST: SettingEntry[] = COMING_SOON_BOTS.map((b) => ({
  id: `${b.id}.soon`,
  section: b.id,
  label: b.name,
  description: `${b.name} support is coming soon`,
  keywords: [b.name.toLowerCase(), 'bot', 'bots', '3rd party', 'third party', 'coming soon', 'agents'],
}))

export function ComingSoonBotSection({ id }: { id: SettingsSection }): React.JSX.Element | null {
  const bot = COMING_SOON_BOTS.find((b) => b.id === id)
  if (!bot) return null
  return (
    <div className="max-w-xl" data-coming-soon-bot={bot.id} data-settings-id={`${bot.id}.soon`}>
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">{bot.name}</h2>
      <p className="text-xs text-[var(--color-text-muted)]">{comingSoonBotCopy(bot.name)}</p>
    </div>
  )
}

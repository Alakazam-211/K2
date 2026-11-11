// One-shot localStorage migration for retired style ids.
//
// Square became Terminal. Bezel users land on Glass: Graphite on the Yellow
// palette, Porcelain on Veil with Bezel's gold accent. glass-next became Glass,
// keeping its palette, accent and tuning. The pre-paint bootstrap and every incoming id
// already go through the alias table in lib/style-resolve; this moves what's
// SAVED so per-style keys follow.
//
// There is no "done" flag: once the old keys are gone a run finds nothing
// to do, so it stays safe to repeat, and an interrupted run finishes on the
// next boot. Bezel's own dial and gamma keys are orphaned on purpose (they
// tuned a look that is gone).
//
// On the retirement ledger (prd-themes-terminal-glass-v1 §7), removed with
// the alias table.

import { accentStorageKey, isSwitchYellow, legacyAccentStorageKey, parseAccentChoice } from '@/lib/style-accent'
import { STYLE_ID_ALIASES, parseSchemeMode, type SchemeMode } from '@/lib/style-resolve'
import { STYLES, getStyle } from '@/styles.generated'

/** Set once the shared per-style accent keys have moved per palette. */
export const LS_ACCENT_PER_PALETTE_MIGRATED = 'k2.style.migrated-accent-per-palette'
/** The one-time 0.46.11 switch's flag (stores/style.ts has the same name). */
const LS_SWITCHED_TO_DEFAULT_GLASS = 'k2.style.migrated-0-46-11-glass'

const PER_STYLE_PREFIXES = ['k2.textGamma.', 'k2.dial.'] as const
const PALETTE_KEYS = ['k2.palette', 'k2.palette.dark', 'k2.palette.light'] as const

/** Where a per-style key moves, or null if it stays. */
function movedKey(key: string): string | null {
  for (const [id, alias] of STYLE_ID_ALIASES) {
    if (alias.keepsTuning && key === legacyAccentStorageKey(id)) return legacyAccentStorageKey(alias.to)
  }
  for (const prefix of PER_STYLE_PREFIXES) {
    if (!key.startsWith(prefix)) continue
    const rest = key.slice(prefix.length)
    for (const [id, alias] of STYLE_ID_ALIASES) {
      if (alias.keepsTuning && rest.startsWith(`${id}.`)) {
        return `${prefix}${alias.to}.${rest.slice(id.length + 1)}`
      }
    }
  }
  return null
}

function allKeys(storage: Storage): string[] {
  const keys: string[] = []
  for (let i = 0; i < storage.length; i++) {
    const key = storage.key(i)
    if (key !== null) keys.push(key)
  }
  return keys
}

/** Give the light palette a retired light palette moved to the accent it
 *  had, for someone who used it (light or auto), unless a colour is already
 *  chosen there. Callers run this only while moving a saved retired id, so
 *  it happens once. Never throws. */
export function seedRetiredStyleAccent(savedId: string, schemeMode: SchemeMode): void {
  const alias = STYLE_ID_ALIASES.get(savedId)
  if (!alias?.accentOnLight || schemeMode === 'dark' || typeof localStorage === 'undefined') return
  const light = alias.palettes?.light ?? getStyle(alias.to)?.defaultPalettes.light
  if (!light) return
  try {
    const key = accentStorageKey(alias.to, light)
    if (parseAccentChoice(localStorage.getItem(key)) === null) localStorage.setItem(key, alias.accentOnLight)
  } catch {
    // Privacy-mode / storage failure: they land on the palette's own accent.
  }
}

/** Rewrite a saved retired style id and move its per-style keys. A value
 *  already at the new key wins. Never throws: a storage failure only costs
 *  this boot's clean-up, and the aliases still apply. */
export function migrateRetiredStyleKeys(): void {
  if (typeof localStorage === 'undefined') return
  try {
    const saved = localStorage.getItem('k2.style')
    const alias = saved === null ? undefined : STYLE_ID_ALIASES.get(saved)
    if (alias) {
      const mode = parseSchemeMode(localStorage.getItem('k2.scheme'))
      localStorage.setItem('k2.style', alias.to)
      seedRetiredStyleAccent(saved as string, mode)
      if (!alias.keepsTuning) localStorage.removeItem('k2.gaps')
      if (alias.palettes) {
        localStorage.setItem('k2.palette', alias.palettes[mode === 'light' ? 'light' : 'dark'])
        localStorage.setItem('k2.palette.dark', alias.palettes.dark)
        localStorage.setItem('k2.palette.light', alias.palettes.light)
      } else {
        // A palette the new style doesn't have goes; the store falls to the
        // style's per-scheme default and rewrites these keys on first apply.
        const palettes = getStyle(alias.to)?.palettes ?? []
        for (const key of PALETTE_KEYS) {
          const palette = localStorage.getItem(key)
          if (palette !== null && !palettes.some((p) => p.id === palette)) localStorage.removeItem(key)
        }
      }
    }
    for (const key of allKeys(localStorage)) {
      const next = movedKey(key)
      if (next === null) continue
      const value = localStorage.getItem(key)
      if (value !== null && localStorage.getItem(next) === null) localStorage.setItem(next, value)
      localStorage.removeItem(key)
    }
  } catch {
    // Privacy-mode / storage failure: the aliases still resolve the id.
  }
}

/**
 * Move each style's one shared accent (`k2.accent.<styleId>`) onto every
 * palette of that style that has none yet (`k2.accent.<styleId>.<paletteId>`),
 * so a person's look doesn't change, then remove the shared key. Once,
 * behind LS_ACCENT_PER_PALETTE_MIGRATED, and safe to repeat: a re-run finds
 * no shared keys and a palette's own key always wins.
 *
 * Glass's shared key holding Yellow on a client past the one-time 0.46.11
 * switch came from that switch, not from the person (it forced Yellow onto
 * Obsidian, Veil and Yellow alike): it is dropped, so each palette shows its
 * own accent. Runs before the switch, so a client meeting both at once keeps
 * a Yellow it chose. Never throws.
 */
export function migrateSharedAccentKeys(): void {
  if (typeof localStorage === 'undefined') return
  try {
    if (localStorage.getItem(LS_ACCENT_PER_PALETTE_MIGRATED) === '1') return
    const switched = localStorage.getItem(LS_SWITCHED_TO_DEFAULT_GLASS) === '1'
    for (const style of STYLES) {
      const oldKey = legacyAccentStorageKey(style.id)
      const raw = localStorage.getItem(oldKey)
      if (raw === null) continue
      const choice = parseAccentChoice(raw)
      const fromSwitch = style.id === 'glass' && switched && isSwitchYellow(raw)
      if (choice !== null && !fromSwitch) {
        for (const p of style.palettes) {
          const key = accentStorageKey(style.id, p.id)
          if (localStorage.getItem(key) === null) localStorage.setItem(key, choice)
        }
      }
      localStorage.removeItem(oldKey)
    }
    localStorage.setItem(LS_ACCENT_PER_PALETTE_MIGRATED, '1')
  } catch {
    // Storage failure: the next boot finishes it; until then a palette with
    // no key shows its own accent.
  }
}

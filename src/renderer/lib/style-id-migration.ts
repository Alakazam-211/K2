// One-shot localStorage migration for retired style ids.
//
// Square became Terminal and Bezel folded into it. The pre-paint bootstrap
// and every incoming id already go through the alias table in
// lib/style-resolve; this moves what's SAVED so per-style keys follow.
//
// There is no "done" flag: once the old keys are gone a run finds nothing
// to do, so it stays safe to repeat, and an interrupted run finishes on the
// next boot. Bezel's own dial and gamma keys are orphaned on purpose (they
// tuned a look that is gone).
//
// On the retirement ledger (prd-themes-terminal-glass-v1 §7), removed with
// the alias table.

import { STYLE_ID_ALIASES } from '@/lib/style-resolve'
import { getStyle } from '@/styles.generated'

const PER_STYLE_PREFIXES = ['k2.textGamma.', 'k2.dial.'] as const
const PALETTE_KEYS = ['k2.palette', 'k2.palette.dark', 'k2.palette.light'] as const

/** Where a per-style key moves, or null if it stays. */
function movedKey(key: string): string | null {
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

/** Rewrite a saved retired style id and move its per-style keys. A value
 *  already at the new key wins. Never throws: a storage failure only costs
 *  this boot's clean-up, and the aliases still apply. */
export function migrateRetiredStyleKeys(): void {
  if (typeof localStorage === 'undefined') return
  try {
    const saved = localStorage.getItem('k2.style')
    const alias = saved === null ? undefined : STYLE_ID_ALIASES.get(saved)
    if (alias) {
      localStorage.setItem('k2.style', alias.to)
      if (!alias.keepsTuning) localStorage.removeItem('k2.gaps')
      // A palette the new style doesn't have goes; the store falls to the
      // style's per-scheme default and rewrites these keys on first apply.
      const palettes = getStyle(alias.to)?.palettes ?? []
      for (const key of PALETTE_KEYS) {
        const palette = localStorage.getItem(key)
        if (palette !== null && !palettes.some((p) => p.id === palette)) localStorage.removeItem(key)
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

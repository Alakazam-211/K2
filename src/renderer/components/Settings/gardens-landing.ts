// Where Settings → Gardens opens (0.46.3, Rosson 2026-10-23). Local Data
// and External Data aren't Settings pages of their own any more on a
// client with Gardens: each Garden's pane shows its own, and "Not in a
// Garden" shows the rest. A deep link or a search result that used to land
// on `zen-widget-data` / `data-sources` lands on Gardens instead, with the
// Garden selected when it knows it, else on "Not in a Garden".
//
// One-shot view state of this window (which rail entry to open), not
// daemon truth: the page takes it once and clears it.

import { create } from 'zustand'
import type { SettingsSection } from '@/stores/settings'

export interface GardensLanding {
  /** The Garden to open, when the caller knows it. */
  garden: string | null
  /** Which part the caller was after. */
  part: 'data' | 'sources' | null
}

interface GardensLandingState {
  landing: GardensLanding | null
}

export const useGardensLandingStore = create<GardensLandingState>(() => ({ landing: null }))

export function requestGardensLanding(landing: GardensLanding): void {
  useGardensLandingStore.setState({ landing })
}

/** The pending landing, cleared. */
export function takeGardensLanding(): GardensLanding | null {
  const l = useGardensLandingStore.getState().landing
  if (l) useGardensLandingStore.setState({ landing: null })
  return l
}

/** The sections that fold into Settings → Gardens on a client with Gardens. */
export const GARDEN_DATA_SECTIONS: ReadonlySet<SettingsSection> = new Set<SettingsSection>(['zen-widget-data', 'data-sources'])

/** Where a section (and, for a search result, its Garden) lands (pure).
 *  `gardensPage`: this client shows Settings → Gardens (desktop with Zen).
 *  Without it, every section lands on itself (web, Windows: External Data
 *  stays its own page under Network). */
export function gardensLandingFor(
  section: SettingsSection,
  garden: string | null | undefined,
  gardensPage: boolean,
): { section: SettingsSection; landing: GardensLanding | null } {
  if (!gardensPage) return { section, landing: null }
  if (GARDEN_DATA_SECTIONS.has(section)) {
    return { section: 'zen-gardens', landing: { garden: garden ?? null, part: section === 'data-sources' ? 'sources' : 'data' } }
  }
  if (section === 'zen-gardens' && garden) return { section, landing: { garden, part: null } }
  return { section, landing: null }
}

/** Which rail entry a landing opens (pure): its Garden when that Garden
 *  exists, else "Not in a Garden" (where data and sources no Garden uses
 *  are) for a data or sources landing, else null (leave the page as is). */
export function gardensLandingSelection(
  landing: GardensLanding,
  gardenIds: readonly string[],
): { kind: 'garden'; id: string } | { kind: 'unplaced' } | null {
  if (landing.garden && gardenIds.includes(landing.garden)) return { kind: 'garden', id: landing.garden }
  if (landing.part) return { kind: 'unplaced' }
  return null
}

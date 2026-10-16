// Per-window Wiki view choice: Wiki (standard reading view) | 2D graph |
// 3D graph (prd-wiki-3d-view-v1 §6.6, Standard view §12). Thin-client view
// state, not daemon SSOT: two clients may view the same wiki differently.
// Same storage rule as window chrome (`lib/window-chrome.ts`): localStorage
// on desktop, sessionStorage on the web client, no `storage`-event sync.

import { getWindowLabel } from '@/lib/window-chrome'
import { isWebClient } from '@/lib/is-web'

export type WikiView = 'wiki' | '2d' | '3d'

export const WIKI_VIEWS: readonly WikiView[] = ['wiki', '2d', '3d']

/** Every new window opens the 2D graph (Rosson 2026-10-16); a window's own pick wins after that. */
export const DEFAULT_WIKI_VIEW: WikiView = '2d'

export const WIKI_VIEW_KEY_PREFIX = 'k2.wikiView.'

export function wikiViewKey(label: string): string {
  return `${WIKI_VIEW_KEY_PREFIX}${label}`
}

export function isWikiView(v: unknown): v is WikiView {
  return v === 'wiki' || v === '2d' || v === '3d'
}

function viewStorage(): Storage | null {
  try {
    if (isWebClient()) {
      return typeof sessionStorage === 'undefined' ? null : sessionStorage
    }
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

/** This window's saved view; a missing or bad value (or no storage) reads as the default. */
export function readWikiViewPref(label: string = getWindowLabel()): WikiView {
  try {
    const raw = viewStorage()?.getItem(wikiViewKey(label))
    return isWikiView(raw) ? raw : DEFAULT_WIKI_VIEW
  } catch {
    return DEFAULT_WIKI_VIEW
  }
}

/** Save this window's view. Never throws (a full or blocked storage just doesn't persist). */
export function writeWikiViewPref(view: WikiView, label: string = getWindowLabel()): void {
  try {
    viewStorage()?.setItem(wikiViewKey(label), view)
  } catch {
    /* storage blocked or full: the choice lives for this session only */
  }
}

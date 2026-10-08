// DAY-0 PLACEHOLDER, hand-written 2026-10-08 (prd-zen-user-widgets-v2 §15).
// B1's `contract-gen` (UWA11) replaces this whole file with generated output
// from `crates/k2-core/src/contract/catalog.json`, keeping these export
// names and shapes. B4 imports it now so the bridge never waits on the
// generator. Once generated, never edit by hand: TUWA4 fails on drift.

import type { CatalogReach, CatalogVerbKind } from '../contract/catalog-types'

/** The catalog version this table was generated from (0 = placeholder). */
export const ZEN_CATALOG_VERSION = 0

/**
 * Every catalog row with a `builtin` or `registered` renderer binding,
 * verb → cap. Must equal today's hand table in `zen-bridge.ts` key for key
 * (TUWA5), so the switch-over changes nothing for built-ins.
 */
export const ZEN_VERBS = {
  'agents.list': 'agents:read',
  'agents.subscribe': 'agents:read',
  'agents.add': 'agents:add',
  'agents.home': 'agents:read',
  'agents.setHome': 'agents:read',
  'agents.local': 'agents:read',
  'presence.get': 'presence:read',
  'presence.subscribe': 'presence:read',
  'conversation.open': 'agents:read',
  'conversation.close': 'agents:read',
  'thread.read': 'thread:read',
  'thread.subscribe': 'thread:read',
  'thread.markRead': 'thread:read',
  'thread.post': 'thread:post',
  'thread.answer': 'thread:post',
  'thread.void': 'thread:post',
  'compose.draft': 'thread:post',
  'focusGroups.get': 'agents:read',
  'focusGroups.set': 'agents:read',
  'focusGroups.subscribe': 'agents:read',
  'app.open': 'app:navigate',
  'app.current': 'app:navigate',
  'app.subscribeCurrent': 'app:navigate',
  'app.badges': 'app:navigate',
  'app.subscribe': 'app:navigate',
  'homes.list': 'agents:read',
  'gardens.list': null,
  'gardens.current': null,
  'gardens.switch': null,
  'gardens.create': 'gardens:manage',
  'gardens.rename': 'gardens:manage',
  'gardens.delete': 'gardens:manage',
  'gardens.empty': 'gardens:template',
  'gardens.useTemplate': 'gardens:template',
  'zen.exit': null,
  'controls.bind': null,
  'theme.get': null,
} as const satisfies Record<string, string | null>

/** One custom-widget verb: its cap, portable or local, and how it answers. */
export interface ZenCustomVerbRow {
  readonly cap: 'agents:read' | 'presence:read' | 'thread:read' | 'thread:post' | null
  readonly reach: CatalogReach
  readonly kind: CatalogVerbKind
}

/**
 * The custom-widget allowlist (UW16 as amended by UWB10): catalog rows whose
 * `exposure` includes `widget`. Its own table, never `ZEN_VERBS`: custom
 * widgets never get `controls.bind`, `zen.exit`, `homes.list`,
 * `agents.home/setHome/local/add`, `gardens.create/rename/delete`, `app.*`,
 * `thread.void` or `thread.markRead`.
 */
export const ZEN_CUSTOM_VERBS = {
  'gardens.list': { cap: null, reach: 'local', kind: 'call' },
  'gardens.current': { cap: null, reach: 'local', kind: 'call' },
  'gardens.switch': { cap: null, reach: 'local', kind: 'call' },
  'theme.get': { cap: null, reach: 'local', kind: 'call' },
  'theme.changed': { cap: null, reach: 'local', kind: 'event' },
  'agents.list': { cap: 'agents:read', reach: 'portable', kind: 'call' },
  'agents.subscribe': { cap: 'agents:read', reach: 'portable', kind: 'subscribe' },
  'conversation.open': { cap: 'agents:read', reach: 'local', kind: 'call' },
  'conversation.close': { cap: 'agents:read', reach: 'local', kind: 'call' },
  'presence.get': { cap: 'presence:read', reach: 'local', kind: 'call' },
  'presence.subscribe': { cap: 'presence:read', reach: 'local', kind: 'subscribe' },
  'thread.read': { cap: 'thread:read', reach: 'portable', kind: 'call' },
  'thread.subscribe': { cap: 'thread:read', reach: 'portable', kind: 'subscribe' },
  'thread.post': { cap: 'thread:post', reach: 'portable', kind: 'call' },
  'thread.answer': { cap: 'thread:post', reach: 'portable', kind: 'call' },
  'compose.draft': { cap: 'thread:post', reach: 'local', kind: 'call' },
} as const satisfies Record<string, ZenCustomVerbRow>

export type ZenCustomVerb = keyof typeof ZEN_CUSTOM_VERBS

// prd-zen-mode-v1 S6 and prd-zen-gardens-v1 G11, G28, G58 — the built-in
// pieces of the two Garden templates, plugged into the skeleton's
// registries: the Agents, Conversation and empty-Garden widgets
// (`registerZenWidget`), the data verbs (`installZenDataVerbs`), and
// `agents.add` (`installZenAddAgentVerb`). K2's controls (the Garden
// switcher, the Zen toggle, usage, theme, menus) are the chrome registry's
// own entries (`zen-registry.tsx`, prd-zen-freeform-chrome FC43); Add agent
// is the Agents widget's last row. Built-ins go through the same bridge v2
// user widgets will.

import { installZenDataVerbs } from '@/lib/zen/zen-data'
import { installZenAddAgentVerb } from '@/lib/zen/zen-add-agent'
import { registerZenWidget } from '../zen-registry'
import { ZenAgentsWidget } from './ZenAgentsWidget'
import { ZenConversationWidget } from './ZenConversationWidget'
import { ZenGardenEmptyWidget } from './ZenGardenEmptyWidget'
import { ZenNavRailWidget } from './ZenNavRailWidget'
import { ZenProjectsViewWidget } from './ZenProjectsViewWidget'
import { ZenTicketsViewWidget } from './ZenTicketsViewWidget'
import { ZEN_PROJECTS_VIEW_KIND, ZEN_TICKETS_VIEW_KIND } from '@/lib/zen/zen-rail-views'
import { installZenAppNavVerbs } from '@/lib/zen/zen-app-nav'

/** Install every built-in. Returns the uninstall. */
export function installZenBuiltins(): () => void {
  const offs = [
    installZenDataVerbs(),
    installZenAddAgentVerb(),
    registerZenWidget('agents', ZenAgentsWidget),
    registerZenWidget('conversation', ZenConversationWidget),
    registerZenWidget('garden-empty', ZenGardenEmptyWidget),
    registerZenWidget('nav-rail', ZenNavRailWidget),
    // The rail's K2 views (Garden 1's Projects and Tickets, in Zen).
    registerZenWidget(ZEN_PROJECTS_VIEW_KIND, ZenProjectsViewWidget),
    registerZenWidget(ZEN_TICKETS_VIEW_KIND, ZenTicketsViewWidget),
    installZenAppNavVerbs(),
  ]
  return () => {
    for (const off of offs.reverse()) off()
  }
}

let installed: (() => void) | null = null

/** Once per webview (App chunk). */
export function ensureZenBuiltins(): void {
  if (!installed) installed = installZenBuiltins()
}

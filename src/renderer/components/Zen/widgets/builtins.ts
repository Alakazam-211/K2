// prd-zen-mode-v1 S6 and prd-zen-gardens-v1 G11, G28, G58 — the built-in
// pieces of the two Garden templates, plugged into the skeleton's
// registries: the Agents, Conversation and empty-Garden widgets
// (`registerZenWidget`), the templates' own Garden switcher, Zen toggle and
// drag area (`registerZenTemplateControls`, one top band; Add agent is the
// Agents widget's last row), the data verbs
// (`installZenDataVerbs`), and `agents.add` (`installZenAddAgentVerb`).
// Built-ins go through the same bridge v2 user widgets will.

import { installZenDataVerbs } from '@/lib/zen/zen-data'
import { installZenAddAgentVerb } from '@/lib/zen/zen-add-agent'
import { BLANK_TEMPLATE_ID, TEXTING_TEMPLATE_ID } from '@/lib/zen/zen-page'
import { registerZenTemplateControls, registerZenWidget } from '../zen-registry'
import { ZenAgentsWidget } from './ZenAgentsWidget'
import { ZenConversationWidget } from './ZenConversationWidget'
import { ZenGardenEmptyWidget } from './ZenGardenEmptyWidget'
import { ZenNavRailWidget } from './ZenNavRailWidget'
import { installZenAppNavVerbs } from '@/lib/zen/zen-app-nav'
import { ZenTextingControls } from './ZenTextingControls'

/** Install every built-in. Returns the uninstall. */
export function installZenBuiltins(): () => void {
  const offs = [
    installZenDataVerbs(),
    installZenAddAgentVerb(),
    registerZenWidget('agents', ZenAgentsWidget),
    registerZenWidget('conversation', ZenConversationWidget),
    registerZenWidget('garden-empty', ZenGardenEmptyWidget),
    registerZenWidget('nav-rail', ZenNavRailWidget),
    installZenAppNavVerbs(),
    registerZenTemplateControls(TEXTING_TEMPLATE_ID, ZenTextingControls),
    registerZenTemplateControls(BLANK_TEMPLATE_ID, ZenTextingControls),
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

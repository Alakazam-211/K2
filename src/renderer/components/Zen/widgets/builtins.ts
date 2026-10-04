// prd-zen-mode-v1 S6 — the built-in pieces of `k2.texting@1`, plugged into
// the S4 skeleton's registries: the Agents and Conversation widgets
// (`registerZenWidget`), the template's own Home switcher, Zen toggle and
// drag area (`registerZenTemplateControls`, with its bottom-left footer: Zen
// toggle + Add agent), the v1 data verbs plus the ⌘1–9 row select
// (`installZenDataVerbs`), and `agents.add` (`installZenAddAgentVerb`). Built-ins go through the same
// bridge v2 user widgets will.

import { installZenDataVerbs } from '@/lib/zen/zen-data'
import { installZenAddAgentVerb } from '@/lib/zen/zen-add-agent'
import { BUILTIN_TEMPLATE_ID } from '@/lib/zen/zen-page'
import { registerZenTemplateControls, registerZenWidget } from '../zen-registry'
import { ZenAgentsWidget } from './ZenAgentsWidget'
import { ZenConversationWidget } from './ZenConversationWidget'
import { ZenTextingControls, ZenTextingFooter } from './ZenTextingControls'

/** Install every built-in. Returns the uninstall. */
export function installZenBuiltins(): () => void {
  const offs = [
    installZenDataVerbs(),
    installZenAddAgentVerb(),
    registerZenWidget('agents', ZenAgentsWidget),
    registerZenWidget('conversation', ZenConversationWidget),
    registerZenTemplateControls(BUILTIN_TEMPLATE_ID, ZenTextingControls, ZenTextingFooter),
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

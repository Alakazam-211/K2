// prd-zen-mode-v1 Z1, Z10, Z27 and prd-zen-gardens-v1 G11, G58 — what
// draws a resolved Garden page.
//
// The page is data (layout + widgets + controls). Two registries turn it
// into React:
//   - widgets by `kind` (`agents`, `conversation`, `garden-empty`). The
//     built-ins register with `registerZenWidget`; until then a placeholder
//     fills the column.
//   - template controls by template id (`k2.texting@1`, `k2.blank@1`). The
//     template draws its own Zen toggle, Garden switcher and drag strip and
//     binds them through the bridge. One component set serves both
//     templates and safe mode (`widgets/ZenTextingControls`). A template has
//     a top band and, optionally, a footer that ZenPage draws under the
//     first column (the bottom-left corner): the Zen toggle, plus Add agent
//     on `k2.texting@1`.
// Every widget gets only a `ZenWidgetBridge` built with its declared caps.

import type { ComponentType } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { BLANK_TEMPLATE_ID, TEXTING_TEMPLATE_ID } from '@/lib/zen/zen-page'
import { ZenBlankFooter, ZenTextingControls, ZenTextingFooter } from './widgets/ZenTextingControls'

export interface ZenWidgetProps {
  decl: ZenWidgetDecl
  bridge: ZenWidgetBridge
}

export interface ZenTemplateControlsProps {
  /** The template's bridge: the no-cap verbs (gardens.list / current /
   *  switch, zen.exit, controls.bind, theme.get) plus `agents.add` (cap
   *  `agents:add`) and `gardens.create` (cap `gardens:manage`), granted by
   *  K2 to the template's own controls. */
  bridge: ZenWidgetBridge
}

/** Caps K2 grants the template's own controls. */
export const ZEN_TEMPLATE_CONTROL_CAPS: readonly string[] = ['agents:add', 'gardens:manage']

interface TemplateParts {
  top: ComponentType<ZenTemplateControlsProps>
  footer: ComponentType<ZenTemplateControlsProps> | null
}

/** Placeholder until S6 registers the real widget. */
function PlaceholderWidget({ decl }: ZenWidgetProps): React.JSX.Element {
  const title =
    decl.kind === 'agents' ? 'Agents' : decl.kind === 'conversation' ? 'Conversation' : decl.kind === 'garden-empty' ? 'Garden' : decl.kind
  return (
    <div
      className="flex h-full w-full items-center justify-center"
      data-zen-widget={decl.kind}
      data-zen-widget-placeholder=""
      style={{ color: 'var(--zen-text-muted)' }}
    >
      <span style={{ fontSize: 'var(--zen-font-size)' }}>{title}</span>
    </div>
  )
}

const widgets = new Map<string, ComponentType<ZenWidgetProps>>()
const templateControls = new Map<string, TemplateParts>([
  [TEXTING_TEMPLATE_ID, { top: ZenTextingControls, footer: ZenTextingFooter }],
  [BLANK_TEMPLATE_ID, { top: ZenTextingControls, footer: ZenBlankFooter }],
])

/** S6 plug-in point: the component for widget `kind`. Returns the unregister. */
export function registerZenWidget(kind: string, component: ComponentType<ZenWidgetProps>): () => void {
  widgets.set(kind, component)
  return () => {
    if (widgets.get(kind) === component) widgets.delete(kind)
  }
}

export function zenWidgetFor(kind: string): ComponentType<ZenWidgetProps> {
  return widgets.get(kind) ?? PlaceholderWidget
}

/** S6 / v2 plug-in point: the controls a template draws — its top band
 *  and, optionally, its footer under the first column. Registering without
 *  a footer means none (the top band carries every control). */
export function registerZenTemplateControls(
  templateId: string,
  component: ComponentType<ZenTemplateControlsProps>,
  footer: ComponentType<ZenTemplateControlsProps> | null = null,
): () => void {
  const prev = templateControls.get(templateId)
  const parts: TemplateParts = { top: component, footer }
  templateControls.set(templateId, parts)
  return () => {
    if (templateControls.get(templateId) !== parts) return
    if (prev) templateControls.set(templateId, prev)
    else templateControls.delete(templateId)
  }
}

/** The template's controls; an unknown template draws none (and so fails
 *  the required-controls check into safe mode). */
export function zenTemplateControlsFor(templateId: string): ComponentType<ZenTemplateControlsProps> | null {
  return templateControls.get(templateId)?.top ?? null
}

/** The template's footer (under the first column), if it has one. */
export function zenTemplateFooterFor(templateId: string): ComponentType<ZenTemplateControlsProps> | null {
  return templateControls.get(templateId)?.footer ?? null
}

/** The built-in template's controls (safe mode always uses these). */
export const BUILTIN_TEMPLATE_CONTROLS: ComponentType<ZenTemplateControlsProps> = ZenTextingControls

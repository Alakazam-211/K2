// prd-zen-mode-v1 Z1, Z10, Z27 and prd-zen-gardens-v1 G11, G58 — what
// draws a resolved Garden page.
//
// The page is data (layout + widgets + controls). Two registries turn it
// into React:
//   - widgets by `kind` (`agents`, `conversation`, `garden-empty`). The
//     built-ins register with `registerZenWidget`; until then a placeholder
//     fills the column.
//   - template controls by template id (`k2.texting@1`, `k2.blank@1`). The
//     template draws its own Zen toggle, Garden switcher and drag strip in
//     its top band and binds them through the bridge. One component set
//     serves both templates and safe mode (`widgets/ZenTextingControls`).
//     Add agent is the Agents widget's own last row, not a template control.
// Every widget gets only a `ZenWidgetBridge` built with its declared caps.
// A RAIL kind (`nav-rail`) is drawn as a thin strip at the left edge of its
// column, outside the column's box, and takes no share of the box.

import type { ComponentType } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { BLANK_TEMPLATE_ID, TEXTING_TEMPLATE_ID } from '@/lib/zen/zen-page'
import { ZenTextingControls } from './widgets/ZenTextingControls'

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

/** Placeholder until S6 registers the real widget. */
function PlaceholderWidget({ decl }: ZenWidgetProps): React.JSX.Element {
  const title =
    decl.kind === 'agents'
      ? 'Agents'
      : decl.kind === 'conversation'
        ? 'Conversation'
        : decl.kind === 'garden-empty'
          ? 'Garden'
          : decl.kind
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

/** Widget kinds drawn as a strip at the left edge of their column. */
export const ZEN_RAIL_KINDS: ReadonlySet<string> = new Set(['nav-rail'])
const templateControls = new Map<string, ComponentType<ZenTemplateControlsProps>>([
  [TEXTING_TEMPLATE_ID, ZenTextingControls],
  [BLANK_TEMPLATE_ID, ZenTextingControls],
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

/** S6 / v2 plug-in point: the controls a template draws (its top band). */
export function registerZenTemplateControls(
  templateId: string,
  component: ComponentType<ZenTemplateControlsProps>,
): () => void {
  const prev = templateControls.get(templateId)
  templateControls.set(templateId, component)
  return () => {
    if (templateControls.get(templateId) !== component) return
    if (prev) templateControls.set(templateId, prev)
    else templateControls.delete(templateId)
  }
}

/** The template's controls; an unknown template draws none (and so fails
 *  the required-controls check into safe mode). */
export function zenTemplateControlsFor(templateId: string): ComponentType<ZenTemplateControlsProps> | null {
  return templateControls.get(templateId) ?? null
}

/** The built-in template's controls (safe mode always uses these). */
export const BUILTIN_TEMPLATE_CONTROLS: ComponentType<ZenTemplateControlsProps> = ZenTextingControls

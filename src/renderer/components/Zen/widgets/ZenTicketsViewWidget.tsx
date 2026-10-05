// Rosson 2026-10-04 — the nav rail's Tickets view in Garden 1: the app's
// Tickets page layout (list + item view, `TicketsPageBoard`: the same
// board, data, filters and HTML brief frame) inside Zen, in a liquid glass
// look.
//
//   - Chat only: the ticket's rail shows its Thread, never the asking
//     session's terminal (`TicketRailTerminalContext` = false).
//   - Liquid glass: the list, the item and the chat are frosted panels
//     (translucent Zen surface, backdrop blur, soft light border) over a
//     soft wash of the theme's own accent colours, so they read in light
//     and dark and in every Zen theme. Colours are Zen tokens only (the
//     board's Styles variables are already Zen tokens under the Zen root's
//     shield). Reduced transparency: solid surfaces, no blur.
//   - Its column has no box (`ZEN_UNBOXED_KINDS`): the panels are the box.
//
// It is K2's own view, not a Garden widget: it reads the Tickets stores
// directly, like the Tickets page.

import { TicketsPageBoard } from '@/components/Feedback/FeedbackPage'
import { TicketRailTerminalContext } from '@/components/Feedback/TicketAgentRail'
import type { ZenWidgetProps } from '../zen-registry'

const GLASS = 'color-mix(in srgb, var(--zen-surface) 62%, transparent)'
const GLASS_EDGE = 'color-mix(in srgb, var(--zen-border) 55%, color-mix(in srgb, var(--zen-text) 14%, transparent))'

/** The glass panels, scoped to this view. Panel selectors are the board's
 *  own test ids, so the board itself is not forked. */
export const ZEN_TICKETS_GLASS_CSS = `
[data-zen-root] [data-zen-tickets] {
  --zen-glass: ${GLASS};
  --zen-glass-edge: ${GLASS_EDGE};
  --zen-glass-blur: blur(22px) saturate(1.5);
  background:
    radial-gradient(60% 55% at 12% 8%, color-mix(in srgb, var(--zen-accent) 16%, transparent), transparent 70%),
    radial-gradient(55% 60% at 92% 92%, color-mix(in srgb, var(--zen-working) 12%, transparent), transparent 70%),
    radial-gradient(40% 45% at 70% 20%, color-mix(in srgb, var(--zen-needs-you) 8%, transparent), transparent 70%);
  border-radius: var(--zen-radius);
}
[data-zen-root] [data-zen-tickets] [data-testid="ticket-board"] { gap: var(--zen-gap); padding: 0; background: transparent; }
[data-zen-root] [data-zen-tickets] [data-testid="ticket-detail-wrap"] { gap: var(--zen-gap); }
[data-zen-root] [data-zen-tickets] [data-testid="ticket-list"],
[data-zen-root] [data-zen-tickets] [data-testid="ticket-list-rail"],
[data-zen-root] [data-zen-tickets] [data-testid="ticket-detail"],
[data-zen-root] [data-zen-tickets] [data-testid="ticket-agent-rail"] {
  background: var(--zen-glass);
  -webkit-backdrop-filter: var(--zen-glass-blur);
  backdrop-filter: var(--zen-glass-blur);
  border: 1px solid var(--zen-glass-edge);
  border-radius: var(--zen-radius);
  box-shadow:
    inset 0 1px 0 color-mix(in srgb, white 22%, transparent),
    0 10px 30px color-mix(in srgb, black 8%, transparent);
  overflow: hidden;
}
[data-zen-root] [data-zen-tickets] [data-testid="ticket-list"] { overflow: visible; }
[data-zen-root] [data-zen-tickets] [data-testid="ticket-detail-header"] { background: transparent; }
[data-zen-root] [data-zen-tickets] [data-testid="ticket-detail"] > div,
[data-zen-root] [data-zen-tickets] [data-testid="ticket-agent-rail"] > div { border-color: var(--zen-glass-edge); }
[data-zen-root] [data-zen-tickets] [data-testid="ticket-board"] > div:last-child > div:only-child:not([data-testid]) {
  border-radius: var(--zen-radius);
  background: var(--zen-glass);
  -webkit-backdrop-filter: var(--zen-glass-blur);
  backdrop-filter: var(--zen-glass-blur);
}
[data-zen-root] [data-zen-tickets] input,
[data-zen-root] [data-zen-tickets] textarea {
  background: color-mix(in srgb, var(--zen-surface-raised) 70%, transparent);
  border-radius: calc(var(--zen-radius) / 2);
}
[data-zen-root] [data-zen-tickets] [role="tablist"] { border-radius: 999px; overflow: hidden; }
@media (prefers-reduced-transparency: reduce) {
  [data-zen-root] [data-zen-tickets] { background: none; }
  [data-zen-root] [data-zen-tickets] [data-testid="ticket-list"],
  [data-zen-root] [data-zen-tickets] [data-testid="ticket-list-rail"],
  [data-zen-root] [data-zen-tickets] [data-testid="ticket-detail"],
  [data-zen-root] [data-zen-tickets] [data-testid="ticket-agent-rail"] {
    background: var(--zen-surface);
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
  }
}
`

export function ZenTicketsViewWidget({ decl }: ZenWidgetProps): React.JSX.Element {
  return (
    <div
      className="flex h-full min-h-0 w-full flex-col"
      data-zen-widget="tickets-view"
      data-zen-widget-id={decl.id}
      data-zen-tickets=""
    >
      <style data-zen-tickets-glass="">{ZEN_TICKETS_GLASS_CSS}</style>
      <TicketRailTerminalContext.Provider value={false}>
        <TicketsPageBoard />
      </TicketRailTerminalContext.Provider>
    </div>
  )
}

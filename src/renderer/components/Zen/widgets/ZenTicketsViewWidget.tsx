// Rosson 2026-10-04 — the nav rail's Tickets view in Garden 1: the app's
// Tickets page layout (list + item view, `TicketsPageBoard`: the same
// board, data, filters and HTML brief frame) inside Zen, in a liquid glass
// look.
//
//   - Chat only: the ticket's rail shows its Thread, never the asking
//     session's terminal (`TicketRailTerminalContext` = false).
//   - Liquid glass: the list, the item and the chat are frosted panels,
//     the shared Zen glass every Zen tile uses (`lib/zen/zen-glass.ts`), so
//     they read in light and dark and in every Zen theme. Colours are Zen
//     tokens only (the board's Styles variables are already Zen tokens
//     under the Zen root's shield). Reduced transparency: solid surfaces,
//     no blur.
//   - Its column has no box (`ZEN_UNBOXED_KINDS`): the panels are the box.
//
// It is K2's own view, not a Garden widget: it reads the Tickets stores
// directly, like the Tickets page.

import { TicketsPageBoard } from '@/components/Feedback/FeedbackPage'
import { TicketRailTerminalContext } from '@/components/Feedback/TicketAgentRail'
import { zenGlassRule } from '@/lib/zen/zen-glass'
import type { ZenWidgetProps } from '../zen-registry'

/** The board's panels: the shared Zen glass (`zen-glass.ts`), on the
 *  board's own test ids, so the board itself is not forked. */
const PANELS = ['ticket-list', 'ticket-list-rail', 'ticket-detail', 'ticket-agent-rail'].map(
  (id) => `[data-zen-root] [data-zen-tickets] [data-testid="${id}"]`,
)

/** The glass panels, scoped to this view. The glass tokens are the Zen
 *  root's (`ZEN_GLASS_TOKENS_CSS`). */
export const ZEN_TICKETS_GLASS_CSS = `
[data-zen-root] [data-zen-tickets] {
  background: transparent; /* Rosson 2026-10-04: no background gradient */
  border-radius: var(--zen-radius);
}
[data-zen-root] [data-zen-tickets] [data-testid="ticket-board"] { gap: var(--zen-gap); padding: 0; background: transparent; }
[data-zen-root] [data-zen-tickets] [data-testid="ticket-detail-wrap"] { gap: var(--zen-gap); }
${zenGlassRule(PANELS)}
${PANELS.join(',\n')} {
  border-radius: var(--zen-radius);
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
  [data-zen-root] [data-zen-tickets] [data-testid="ticket-board"] > div:last-child > div:only-child:not([data-testid]) {
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

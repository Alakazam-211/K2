// The look of "See more" (MessageFold), in the desktop Thread's box only
// (a Garden's Thread themes itself and draws no folds). The phone's look,
// on the desktop's tokens:
//
//   - The clip holds one row more than it shows. On that row: a fade into
//     the bubble's own fill (the accent fill on yours, the agent's grey on
//     theirs) and the pill, drawn over the clip, so measuring never moves
//     the layout. Open: "See less" under the words.
//   - `overflow: clip`, not hidden: a clipped box can't be scrolled by
//     focus, find or scrollIntoView, so the hidden part never slides into
//     the folded view.
//   - The pill is a dark label with white words in every look (the media
//     label's colours): it sits on a bubble, any accent or the agent's grey,
//     not on the page, like a label over a picture.
//   - Glass: a round pill. Dark palettes keep the soft frost edge; the light
//     one (Veil) gets a 3:1 edge against the bubble. Keyboard focus: the
//     accent ring (on your own bubble, the bubble's own text colour, since
//     the accent is its fill).
//   - Terminal: the same layout, chamfered at the chip cut with a 3:1 edge,
//     its fill made solid over the bubble's so the edge never shows through;
//     focus thickens the cut edge as every Thread control's does. The caret
//     swaps with no motion in both looks.

import { CHAMFER_SCOPE, THREAD_ZEN_SCOPE_ATTR } from './chamfer'

const S = `[${THREAD_ZEN_SCOPE_ATTR}]`
const GLASS = `html[data-style="glass"] ${S}`
const VEIL = `html[data-style="glass"][data-palette="veil"] ${S}`

export const THREAD_FOLD_CSS = `
${S} [data-zen-bubble="me"] .thread-fold { --thread-fold-fill: var(--zen-bubble-me); --thread-fold-edge: var(--zen-bubble-me-text); --thread-fold-ring: var(--zen-bubble-me-text); }
${S} [data-zen-bubble="agent"] .thread-fold { --thread-fold-fill: var(--zen-bubble-agent); --thread-fold-edge: var(--zen-control-edge, var(--zen-border)); --thread-fold-ring: var(--zen-focus-ring, var(--zen-accent)); }
${S} .thread-fold { position: relative; }
${S} .thread-fold-clip { overflow: hidden; overflow: clip; }
${S} .thread-fold-fade { position: absolute; left: 0; right: 0; bottom: 0; height: calc(var(--thread-fold-line, 1.6em) * 2.25); background: linear-gradient(to bottom, transparent, var(--thread-fold-fill) 62%); pointer-events: none; }
${S} .thread-fold-toggle { position: relative; display: inline-flex; align-items: center; height: calc(var(--thread-fold-line, 1.6em) * 0.92 + 3px); padding: 0 0.7em; border: 0; border-radius: var(--radius-pill); background: var(--color-media-label-bg); color: var(--color-media-label-text); font: inherit; font-size: var(--zen-type-sm); font-weight: 600; line-height: 1; cursor: pointer; -webkit-user-select: none; user-select: none; }
${S} .thread-fold[data-fold="folded"] { padding-bottom: 5px; }
${S} .thread-fold[data-fold="folded"] .thread-fold-toggle { position: absolute; left: 0; bottom: 0; }
${S} .thread-fold[data-fold="open"] .thread-fold-toggle { margin-top: 0.25em; }
${S} .thread-fold-chevron { flex: none; width: 0.85em; height: 0.85em; margin-left: 0.3em; }
${S} .thread-fold-toggle:active { opacity: 0.6; }
${S} .thread-fold-toggle:focus-visible { outline: 2px solid var(--thread-fold-ring); outline-offset: 2px; }
${GLASS} .thread-fold-toggle { box-shadow: inset 0 0 0 0.5px var(--material-edge); }
${VEIL} .thread-fold-toggle { box-shadow: inset 0 0 0 1px var(--thread-fold-edge); }
${CHAMFER_SCOPE} .thread-fold-toggle[data-chamfer] { --ch-tl: var(--chamfer-chip); --ch-tr: var(--chamfer-chip); --ch-br: var(--chamfer-chip); --ch-bl: var(--chamfer-chip); --ch-fill: linear-gradient(var(--color-media-label-bg), var(--color-media-label-bg)), var(--thread-fold-fill); --ch-edge: var(--thread-fold-edge); --ch-bw: 1px; }
${CHAMFER_SCOPE} .thread-fold-toggle[data-chamfer]:focus-visible { outline: none; --ch-edge: var(--thread-fold-ring) !important; --ch-bw: 2px !important; }
`

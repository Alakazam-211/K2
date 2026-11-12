// How a shared HTML doc fits a dashboard tile (ProjectDashboard's HtmlDocPane).

/** A doc in a narrow tile: a word longer than the tile breaks rather than
 *  running off its right edge (where the frame has no scroll bar to show
 *  it), and anything wider still scrolls. Goes first, so the doc's own
 *  styles win wherever they set these. */
export const DOC_TILE_FIT_CSS = '<style>html{overflow-wrap:anywhere;overflow:auto}</style>'

export function fitToTile(html: string): string {
  // Inside <head> when there is one; never ahead of a doctype (that would
  // put the doc in quirks mode).
  const head = /<head[^>]*>/i.exec(html)
  if (head) return html.slice(0, head.index + head[0].length) + DOC_TILE_FIT_CSS + html.slice(head.index + head[0].length)
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html)
  if (doctype) return doctype[0] + DOC_TILE_FIT_CSS + html.slice(doctype[0].length)
  return DOC_TILE_FIT_CSS + html
}

// The project bar's height: the Projects page's top row beside the nav and
// the chat. The nav's top row and the chat's header take the same height,
// so their bottom lines continue the bar's across the gaps (the nav's top
// row matches the tabs beside it, as on Home and Agents).

/** The project bar's height, its bottom line included (border-box). */
export const PROJECT_BAR_HEIGHT_PX = 48

/** A row level with the bar inside a panel. Glass edges its panels (the
 *  nav tile, the chat drawer) with a rim that sits above the row, so the row
 *  is that much shorter and its bottom line meets the bar's; Terminal's
 *  panels have no top edge. */
export const PROJECT_BAR_ROW_HEIGHT = `calc(${PROJECT_BAR_HEIGHT_PX}px - var(--tile-edge-width, 0px))`

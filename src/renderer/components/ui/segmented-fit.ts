// A Segmented that doesn't fit beside or under its label (a narrow window,
// the app zoomed in): the last resort before it would run past its card.
//
// Rows wrap first: SettingRow, card headers and the bespoke header rows put
// the control on its own line, at its natural width, when it doesn't fit
// beside the label. Only when it is still wider than its line does it fill
// the line with equal-width options (`data-fill`, globals.css), labels cut
// with an ellipsis and the full label as the segment's title. It never wraps
// onto two lines: that would break the sliding highlight.
//
// Measured in CSS px (scrollWidth, clientWidth, computed padding): the same
// at any app zoom.

const px = (v: string): number => parseFloat(v) || 0

function inlineChrome(el: Element): number {
  const s = getComputedStyle(el)
  return px(s.paddingLeft) + px(s.paddingRight) + px(s.borderLeftWidth) + px(s.borderRightWidth)
}

/** The group's width with every label in full, in CSS px. */
export function segmentedNaturalWidth(root: HTMLElement): number {
  let width = inlineChrome(root)
  for (const segment of Array.from(root.querySelectorAll<HTMLElement>(':scope > .k2-segment'))) {
    const label = segment.querySelector<HTMLElement>(':scope > .k2-segment-label')
    width += (label?.scrollWidth ?? segment.scrollWidth) + inlineChrome(segment)
  }
  return width
}

/** The room its line gives it: the parent's content width, in CSS px. */
function lineWidth(root: HTMLElement): number {
  const parent = root.parentElement
  if (!parent) return 0
  const s = getComputedStyle(parent)
  return parent.clientWidth - px(s.paddingLeft) - px(s.paddingRight)
}

/** Fill the line when the labels in full don't fit it; give a cut label its
 *  full text as the title (unless the caller set one). */
export function fitSegmented(root: HTMLElement): void {
  const room = lineWidth(root)
  // No layout (a test DOM, a hidden pane): leave it as it is.
  if (room <= 0) return
  const fill = segmentedNaturalWidth(root) > room + 0.5
  root.toggleAttribute('data-fill', fill)
  for (const segment of Array.from(root.querySelectorAll<HTMLElement>(':scope > .k2-segment'))) {
    const label = segment.querySelector<HTMLElement>(':scope > .k2-segment-label')
    if (!label) continue
    const cut = fill && label.scrollWidth > label.clientWidth + 0.5
    if (cut && !segment.hasAttribute('title')) {
      segment.title = label.textContent ?? ''
      segment.dataset.k2AutoTitle = ''
    } else if (!cut && segment.dataset.k2AutoTitle !== undefined) {
      segment.removeAttribute('title')
      delete segment.dataset.k2AutoTitle
    }
  }
}

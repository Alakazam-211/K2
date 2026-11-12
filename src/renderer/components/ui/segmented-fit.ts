// A Segmented that doesn't fit its line (a narrow window, the app zoomed in).
//
// Rows wrap first: SettingRow, card headers and the bespoke header rows put
// the control on its own line, at its natural width, when it doesn't fit
// beside the label. Each of those lines carries `data-k2-line`.
//
// If the options still don't fit whole on that line, the picker becomes a
// dropdown showing the chosen option (Segmented.tsx renders a SettingDropdown
// with the same options). Cut labels ("W… M… Lo…") can't be read, and equal
// widths can't keep every label whole once the natural widths don't fit (the
// widest label times the count is at least their sum), so there is no
// in-between. It never wraps onto two lines: that would break the sliding
// highlight.
//
// Measured in CSS px (scrollWidth, clientWidth, computed padding): the same
// at any app zoom. While it shows as a dropdown, the group stays laid out in
// a zero-size probe so its natural width can still be measured.

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

/** Where the picker sits: its parent, or the probe's parent while it shows
 *  as a dropdown. */
export function segmentedHost(root: HTMLElement): HTMLElement | null {
  const parent = root.parentElement
  return parent?.classList.contains('k2-segmented-probe') ? parent.parentElement : parent
}

/** The line it can have: the nearest `data-k2-line`, else its host. */
export function segmentedLine(root: HTMLElement): HTMLElement | null {
  return root.closest<HTMLElement>('[data-k2-line]') ?? segmentedHost(root)
}

/** Whether the options can't fit whole on the line; null with no layout (a
 *  test DOM, a hidden pane), where nothing should change. */
export function segmentedNeedsMenu(root: HTMLElement): boolean | null {
  const line = segmentedLine(root)
  if (!line) return null
  const s = getComputedStyle(line)
  const room = line.clientWidth - px(s.paddingLeft) - px(s.paddingRight)
  if (room <= 0) return null
  return segmentedNaturalWidth(root) > room + 0.5
}

/** The plain text of an option's label (a dropdown row shows text). */
export function labelText(node: React.ReactNode): string {
  if (node === null || node === undefined || typeof node === 'boolean') return ''
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(labelText).join('')
  if (typeof node === 'object' && 'props' in node) {
    return labelText((node as { props?: { children?: React.ReactNode } }).props?.children)
  }
  return ''
}

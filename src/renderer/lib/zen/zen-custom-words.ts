// prd-zen-user-widgets-v2 UW5 — K2's own words for a custom widget. The
// widget's own name is only ever shown as its own and cut to length.

/** UW5 length: a widget name is cut to 40 characters. */
export const ZEN_WIDGET_NAME_MAX = 40

/** Cut `text` to `max` characters (by code point), control characters out. */
export function zenClip(text: string, max: number): string {
  // eslint-disable-next-line no-control-regex
  const clean = text.replace(/[\u0000-\u001f\u007f]+/g, ' ').trim()
  const chars = [...clean]
  return chars.length <= max ? clean : `${chars.slice(0, max - 1).join('')}…`
}

/** The widget's own name as K2 shows it (cut to 40). */
export function zenWidgetDisplayName(name: string): string {
  return zenClip(name, ZEN_WIDGET_NAME_MAX) || 'This widget'
}

// prd-zen-user-widgets-v2 UW22, UW23, UWA9 — K2's own words for a custom
// widget's permissions. The cap sentences come from the one shared cap
// table (`k2-caps.generated.ts`, from the catalog); the widget's own words
// (name, description, reasons) are only ever shown labelled as its own and
// cut to length, never as a K2 sentence.

import { K2_CAPS, type UserWidgetCap } from '../k2-caps.generated'

/** UW5 lengths: name 40, description 200, a reason 140. */
export const ZEN_WIDGET_NAME_MAX = 40
export const ZEN_WIDGET_DESCRIPTION_MAX = 200
export const ZEN_WIDGET_REASON_MAX = 140

/** Cut `text` to `max` characters (by code point), control characters out. */
export function zenClip(text: string, max: number): string {
  // eslint-disable-next-line no-control-regex
  const clean = text.replace(/[\u0000-\u001f\u007f]+/g, ' ').trim()
  const chars = [...clean]
  return chars.length <= max ? clean : `${chars.slice(0, max - 1).join('')}…`
}

/** K2's sentence for `cap`, with `{where}` filled ("Work", "all your Homes"). */
export function zenCapSentence(cap: UserWidgetCap, where: string): string {
  const row: { label: string; sentence?: string } = K2_CAPS[cap]
  const sentence = row.sentence ? row.sentence : row.label
  return sentence.replace(/\{where\}/g, where)
}

/** The widget's own name as K2 shows it (in quotes, cut to 40). */
export function zenWidgetDisplayName(name: string): string {
  return zenClip(name, ZEN_WIDGET_NAME_MAX) || 'This widget'
}

/** UW24: "now also asks to …" for caps the grant doesn't hold. */
export function zenAlsoAsksText(missing: readonly UserWidgetCap[]): string {
  const words = missing.map((c) => K2_CAPS[c].label.toLocaleLowerCase())
  return `It now also asks to ${words.join(', ')}.`
}

/** The one-line limits K2 states in every review (UW22). */
export const ZEN_WIDGET_LIMITS_TEXT =
  'It can’t use the internet, see your files, see agents outside what you pick here, or change K2.'

export const ZEN_WIDGET_AUTHOR_TEXT = 'An agent on this computer wrote it. K2 hasn’t checked what it does.'

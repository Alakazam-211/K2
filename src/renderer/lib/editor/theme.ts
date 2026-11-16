// The editors' look (build-editors.md E1a). The default theme, "Follow app",
// reads only the look's tokens (--color-*), so CodeEditor, the live editor
// and the diff follow Terminal and Glass, dark and light, and an agent's
// palette edit, with no reconfigure. The eight named themes
// (./named-themes) and custom themes stay as opt-in choices.

import { EditorView } from '@codemirror/view'
import type { Extension } from '@codemirror/state'
import { HighlightStyle, syntaxHighlighting, defaultHighlightStyle, type TagStyle } from '@codemirror/language'
import { tags as t } from '@lezer/highlight'
import { useCustomThemesStore } from '@/stores/custom-themes'
import { NAMED_EDITOR_THEMES, buildEditorTheme, isNamedTheme, namedHighlightStyle, namedThemeExtension } from './named-themes'

export const FOLLOW_APP_THEME_ID = 'follow-app'
/** The default before Follow app. A setting that still holds it was never
 *  a choice, so it follows the app; K2 Dark itself is now `k2-dark`. */
const LEGACY_DEFAULT_THEME_ID = 'k2so-dark'

/** Settings → Code editor's theme list: Follow app, then the named themes. */
export const EDITOR_THEMES: { id: string; label: string }[] = [
  { id: FOLLOW_APP_THEME_ID, label: 'Follow app (default)' },
  ...NAMED_EDITOR_THEMES,
]

/** The theme a stored setting means: Follow app for none, the old default
 *  or an id K2 no longer has; otherwise the named or custom theme. */
export function resolveEditorThemeId(id: string | null | undefined): string {
  if (!id || id === LEGACY_DEFAULT_THEME_ID) return FOLLOW_APP_THEME_ID
  if (id.startsWith('custom:') || isNamedTheme(id)) return id
  return FOLLOW_APP_THEME_ID
}

/** The editor's surface: the pane's, unless the host sets --editor-bg. */
const SURFACE = 'var(--editor-bg, var(--color-bg))'
const accent = (pct: number): string => `color-mix(in srgb, var(--color-accent-text) ${pct}%, transparent)`
// Spelled out so the Garden shield's scan sees each token (zen-style-shield.test).
const SYNTAX: Record<string, string> = {
  keyword: 'var(--color-syntax-keyword)',
  string: 'var(--color-syntax-string)',
  number: 'var(--color-syntax-number)',
  comment: 'var(--color-syntax-comment)',
  function: 'var(--color-syntax-function)',
  type: 'var(--color-syntax-type)',
  property: 'var(--color-syntax-property)',
  punctuation: 'var(--color-syntax-punctuation)',
}
const syntax = (k: string): string => SYNTAX[k]
const RAINBOW = ['type', 'keyword', 'property', 'function', 'string', 'number']

export const FOLLOW_APP_THEME_SPEC = {
  '&': {
    backgroundColor: SURFACE,
    color: 'var(--color-text-primary)',
    height: '100%',
    '--editor-caret': 'var(--color-accent-text)',
  },
  '&.cm-focused': { outline: 'none' },
  '.cm-content': { caretColor: 'var(--color-accent-text)', padding: '8px 0' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--editor-caret)', borderLeftWidth: '2px' },
  '.cm-selectionBackground, ::selection': { backgroundColor: 'var(--color-selection) !important' },
  '.cm-activeLine': { backgroundColor: 'var(--color-wash-1)' },
  '.cm-activeLineGutter': { backgroundColor: 'var(--color-wash-1)', color: 'var(--color-text-primary)' },
  '.cm-gutters': { backgroundColor: SURFACE, color: 'var(--color-text-muted)', border: 'none' },
  '.cm-lineNumbers .cm-gutterElement': { padding: '0 8px 0 16px', minWidth: '3em', fontSize: '11px' },
  '.cm-scroller': { overflow: 'auto', lineHeight: '1.6' },
  '.cm-matchingBracket': { backgroundColor: accent(20), outline: `1px solid ${accent(45)}` },
  '.cm-nonmatchingBracket': { color: 'var(--color-status-error-text)' },
  '.cm-searchMatch': { backgroundColor: 'var(--color-find-match)' },
  '.cm-searchMatch.cm-searchMatch-selected': { backgroundColor: 'var(--color-find-match-current)' },
  '.cm-selectionMatch': { backgroundColor: accent(14) },
  '.cm-foldGutter .cm-gutterElement': { padding: '0 4px', cursor: 'pointer', color: 'var(--color-text-muted)', fontSize: '11px', transition: 'color 0.15s' },
  '.cm-foldGutter .cm-gutterElement:hover': { color: 'var(--color-text-primary)' },
  '.cm-foldPlaceholder': {
    backgroundColor: 'var(--color-bg-elevated)', border: '1px solid var(--color-border)', color: 'var(--color-text-secondary)',
    padding: '0 6px', margin: '0 4px', borderRadius: 'var(--radius-selector)', cursor: 'pointer',
  },
  '.cm-tooltip': {
    backgroundColor: 'var(--color-bg-elevated)', color: 'var(--color-text-primary)', border: '1px solid var(--color-border)',
    borderRadius: 'var(--radius-box)', boxShadow: 'var(--shadow-2)',
  },
  '.cm-tooltip.cm-tooltip-autocomplete > ul': { fontSize: '11px', maxHeight: '200px' },
  '.cm-tooltip.cm-tooltip-autocomplete > ul > li': { padding: '3px 8px', color: 'var(--color-text-secondary)' },
  '.cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]': { backgroundColor: accent(22), color: 'var(--color-text-primary)' },
  '.cm-completionMatchedText': { color: 'var(--color-accent-text)', textDecoration: 'none' },
  '.cm-panels': { backgroundColor: 'var(--color-bg-surface)', color: 'var(--color-text-primary)' },
  '.cm-panels-top': { borderBottom: '1px solid var(--color-border)' },
  '.cm-panels-bottom': { borderTop: '1px solid var(--color-border)' },
  '.cm-panel.cm-search': { padding: '4px 8px', gap: '4px', fontSize: '11px', color: 'var(--color-text-primary)' },
  '.cm-panel input': {
    backgroundColor: 'var(--color-bg-inset)', border: '1px solid var(--color-control-edge)', color: 'var(--color-text-primary)',
    padding: '2px 6px', borderRadius: 'var(--radius-field)', fontSize: '11px', outline: 'none',
  },
  '.cm-panel input:focus': { borderColor: 'var(--color-accent-text)' },
  '.cm-panel button': {
    backgroundColor: 'var(--color-bg-elevated)', backgroundImage: 'none', border: '1px solid var(--color-control-edge)',
    color: 'var(--color-text-primary)', padding: '2px 8px', borderRadius: 'var(--radius-field)', fontSize: '11px', cursor: 'pointer',
  },
  '.cm-panel button:hover': { backgroundColor: 'var(--color-control-hover)' },
  '.cm-panel label': { color: 'var(--color-text-secondary)', fontSize: '11px' },
  '.cm-panel.cm-search [name=close]': { color: 'var(--color-text-secondary)' },
  '.cm-indentation-marker': { opacity: '0.15' },
  '.cm-indentation-marker.active': { opacity: '0.35' },
  ...Object.fromEntries(RAINBOW.map((k, i) => [`.cm-rainbow-${i}`, { color: syntax(k) }])),
}

/** Code colours from the syntax tokens. Every tag CodeMirror's default style
 *  colours is covered, so its fixed fallback palette never paints here. */
export const FOLLOW_APP_HIGHLIGHT: TagStyle[] = [
  { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword, t.definitionKeyword, t.modifier, t.self, t.bool, t.null], color: syntax('keyword') },
  { tag: [t.string, t.special(t.string), t.regexp, t.escape, t.character, t.docString, t.inserted], color: syntax('string') },
  { tag: [t.number, t.integer, t.float, t.literal, t.atom, t.unit, t.color, t.constant(t.variableName)], color: syntax('number') },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment, t.quote], color: syntax('comment') },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName, t.special(t.variableName)], color: syntax('function') },
  { tag: [t.typeName, t.className, t.namespace, t.labelName, t.tagName], color: syntax('type') },
  { tag: [t.propertyName, t.attributeName, t.definition(t.propertyName)], color: syntax('property') },
  { tag: [t.punctuation, t.operator, t.bracket, t.separator, t.meta, t.processingInstruction, t.contentSeparator, t.derefOperator], color: syntax('punctuation') },
  { tag: [t.variableName, t.definition(t.variableName), t.local(t.variableName)], color: 'var(--color-text-primary)' },
  { tag: t.heading, fontWeight: 'bold', color: 'var(--color-text-primary)' },
  { tag: [t.link, t.url], color: 'var(--color-accent-text)', textDecoration: 'underline' },
  { tag: t.monospace, color: 'var(--color-code-inline)' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strong, fontWeight: 'bold' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  { tag: [t.deleted, t.invalid], color: 'var(--color-status-error-text)' },
]

// One extension each for every look: the tokens change, the theme doesn't.
const followAppTheme: Extension = EditorView.theme(FOLLOW_APP_THEME_SPEC)
const followAppHighlight: Extension = syntaxHighlighting(HighlightStyle.define(FOLLOW_APP_HIGHLIGHT))
const namedHighlights = new Map<string, Extension>()

/** The editor look (surface, gutter, caret, selection, find, panels) for a setting. */
export function getThemeExtension(themeId: string | null | undefined): Extension {
  const id = resolveEditorThemeId(themeId)
  if (id.startsWith('custom:')) {
    const custom = useCustomThemesStore.getState().getTheme(id)
    return custom ? buildEditorTheme(custom.colors, custom.isLight) : followAppTheme
  }
  return id === FOLLOW_APP_THEME_ID ? followAppTheme : namedThemeExtension(id)
}

/** The syntax colours for a setting. */
export function getHighlightExtension(themeId: string | null | undefined): Extension {
  const id = resolveEditorThemeId(themeId)
  if (id.startsWith('custom:')) {
    const custom = useCustomThemesStore.getState().getTheme(id)
    return custom ? [syntaxHighlighting(custom.highlight), syntaxHighlighting(defaultHighlightStyle, { fallback: true })] : followAppHighlight
  }
  if (id === FOLLOW_APP_THEME_ID) return followAppHighlight
  let ext = namedHighlights.get(id)
  if (!ext) {
    ext = [syntaxHighlighting(namedHighlightStyle(id)), syntaxHighlighting(defaultHighlightStyle, { fallback: true })]
    namedHighlights.set(id, ext)
  }
  return ext
}

/** The code font, with or without ligatures. */
export function buildFontFamilyExtension(family: string, ligatures: boolean): Extension {
  const fontStack = `"${family}", "Menlo", "Monaco", "Courier New", monospace`
  const ligatureCSS = ligatures
    ? { fontVariantLigatures: 'normal', fontFeatureSettings: '"liga" 1, "calt" 1' }
    : { fontVariantLigatures: 'none', fontFeatureSettings: '"liga" 0, "calt" 0' }
  return EditorView.theme({
    '&': { fontFamily: fontStack },
    '.cm-scroller': { fontFamily: 'inherit' },
    '.cm-content': ligatureCSS,
    '.cm-line': ligatureCSS,
    '.cm-tooltip.cm-tooltip-autocomplete > ul': { fontFamily: fontStack },
  })
}

/** The cursor's shape and blink. Its colour is the theme's (--editor-caret). */
export function buildCursorExtension(style: string, blink: boolean): Extension {
  const anim: Record<string, string> = blink ? {} : { animationDuration: '0' }
  if (style === 'block') {
    return EditorView.theme({
      '.cm-cursor': { borderLeft: 'none', backgroundColor: 'color-mix(in srgb, var(--editor-caret) 50%, transparent)', width: '0.6em', ...anim },
      '&.cm-focused .cm-cursor': anim,
    })
  }
  if (style === 'underline') {
    return EditorView.theme({
      '.cm-cursor': { borderLeft: 'none', borderBottom: '2px solid var(--editor-caret)', width: '0.6em', height: '0 !important', ...anim },
    })
  }
  return EditorView.theme({ '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--editor-caret)', borderLeftWidth: '2px', ...anim } })
}

// The eight named editor themes: fixed palettes, opt-in from Settings → Code
// editor (build-editors.md E1a). The default is "Follow app" (./theme.ts),
// which reads the look's tokens instead. Custom themes (stores/custom-themes)
// and the theme creator's preview also draw through buildEditorTheme.

import { EditorView } from '@codemirror/view'
import type { Extension } from '@codemirror/state'
import { HighlightStyle } from '@codemirror/language'
import { tags } from '@lezer/highlight'
import type { ThemeColors } from '@/lib/editor-themes'

/** The named themes, in the order Settings lists them after Follow app. */
export const NAMED_EDITOR_THEMES: { id: string; label: string }[] = [
  { id: 'k2-dark', label: 'K2 Dark' },
  { id: 'one-dark', label: 'One Dark' },
  { id: 'dracula', label: 'Dracula' },
  { id: 'nord', label: 'Nord' },
  { id: 'github-dark', label: 'GitHub Dark' },
  { id: 'gruvbox-dark', label: 'Gruvbox Dark' },
  { id: 'ayu-mirage', label: 'Ayu Mirage' },
  { id: 'parchment', label: 'Parchment' },
]

const LIGHT_THEMES = new Set(['parchment'])

/** Bracket colours by depth, for every theme but Follow app. */
export const RAINBOW_HEX = ['#fbbf24', '#c084fc', '#22d3ee', '#f472b6', '#34d399', '#fb923c']

const THEME_COLORS: Record<string, ThemeColors> = {
  'k2-dark': {
    bg: '#0a0a0a', fg: '#e4e4e7', gutterBg: '#0a0a0a', gutterFg: '#555', gutterBorder: '#1a1a1a',
    activeLine: '#ffffff08', selection: '#3b82f633', cursor: '#3b82f6', bracket: '#3b82f633',
    bracketOutline: '#3b82f666', searchMatch: '#b5890066', searchMatchSelected: '#b58900aa',
    accent: '#3b82f6', panelBg: '#141414', panelBorder: '#2a2a2a', tooltipBg: '#141414', tooltipBorder: '#2a2a2a',
  },
  'one-dark': {
    bg: '#282c34', fg: '#abb2bf', gutterBg: '#282c34', gutterFg: '#495162', gutterBorder: '#3b4048',
    activeLine: '#2c313c', selection: '#3e4451', cursor: '#528bff', bracket: '#528bff33',
    bracketOutline: '#528bff66', searchMatch: '#d19a6666', searchMatchSelected: '#d19a66aa',
    accent: '#61afef', panelBg: '#21252b', panelBorder: '#3b4048', tooltipBg: '#21252b', tooltipBorder: '#3b4048',
  },
  'dracula': {
    bg: '#282a36', fg: '#f8f8f2', gutterBg: '#282a36', gutterFg: '#6272a4', gutterBorder: '#44475a',
    activeLine: '#44475a44', selection: '#44475a', cursor: '#f8f8f2', bracket: '#bd93f933',
    bracketOutline: '#bd93f966', searchMatch: '#50fa7b44', searchMatchSelected: '#50fa7b88',
    accent: '#bd93f9', panelBg: '#21222c', panelBorder: '#44475a', tooltipBg: '#21222c', tooltipBorder: '#44475a',
  },
  'nord': {
    bg: '#2e3440', fg: '#d8dee9', gutterBg: '#2e3440', gutterFg: '#4c566a', gutterBorder: '#3b4252',
    activeLine: '#3b425244', selection: '#434c5e', cursor: '#88c0d0', bracket: '#88c0d033',
    bracketOutline: '#88c0d066', searchMatch: '#ebcb8b44', searchMatchSelected: '#ebcb8b88',
    accent: '#88c0d0', panelBg: '#292e39', panelBorder: '#3b4252', tooltipBg: '#292e39', tooltipBorder: '#3b4252',
  },
  'github-dark': {
    bg: '#0d1117', fg: '#c9d1d9', gutterBg: '#0d1117', gutterFg: '#484f58', gutterBorder: '#21262d',
    activeLine: '#161b2244', selection: '#264f78', cursor: '#58a6ff', bracket: '#58a6ff33',
    bracketOutline: '#58a6ff66', searchMatch: '#e3b34166', searchMatchSelected: '#e3b341aa',
    accent: '#58a6ff', panelBg: '#161b22', panelBorder: '#30363d', tooltipBg: '#161b22', tooltipBorder: '#30363d',
  },
  'gruvbox-dark': {
    bg: '#282828', fg: '#ebdbb2', gutterBg: '#282828', gutterFg: '#7c6f64', gutterBorder: '#3c3836',
    activeLine: '#3c383644', selection: '#504945', cursor: '#83a598', bracket: '#83a59833',
    bracketOutline: '#83a59866', searchMatch: '#fabd2e44', searchMatchSelected: '#fabd2e88',
    accent: '#83a598', panelBg: '#1d2021', panelBorder: '#3c3836', tooltipBg: '#1d2021', tooltipBorder: '#3c3836',
  },
  'ayu-mirage': {
    bg: '#242936', fg: '#cccac2', gutterBg: '#242936', gutterFg: '#5c6166', gutterBorder: '#2b313a',
    activeLine: '#2b313a44', selection: '#33415e', cursor: '#ffcc66', bracket: '#ffcc6633',
    bracketOutline: '#ffcc6666', searchMatch: '#e6b45066', searchMatchSelected: '#e6b450aa',
    accent: '#ffcc66', panelBg: '#1f2430', panelBorder: '#2b313a', tooltipBg: '#1f2430', tooltipBorder: '#2b313a',
  },
  'parchment': {
    bg: '#f4f0e8', fg: '#3b3228', gutterBg: '#ece8df', gutterFg: '#9e9688', gutterBorder: '#ddd8ce',
    activeLine: '#e8e3d888', selection: '#d4cfc4', cursor: '#3b3228', bracket: '#7b685433',
    bracketOutline: '#7b685466', searchMatch: '#e6c07b66', searchMatchSelected: '#e6c07baa',
    accent: '#7b6854', panelBg: '#ece8df', panelBorder: '#ddd8ce', tooltipBg: '#ece8df', tooltipBorder: '#ddd8ce',
  },
}


// Each theme's syntax colours (from Zed and the themes' own palettes).
const THEME_HIGHLIGHTS: Record<string, HighlightStyle> = {
  'k2-dark': HighlightStyle.define([
    { tag: tags.keyword, color: '#c678dd' },
    { tag: tags.operator, color: '#56b6c2' },
    { tag: tags.string, color: '#98c379' },
    { tag: tags.number, color: '#d19a66' },
    { tag: tags.bool, color: '#d19a66' },
    { tag: tags.null, color: '#d19a66' },
    { tag: tags.comment, color: '#5c6370', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#61afef' },
    { tag: tags.typeName, color: '#e5c07b' },
    { tag: tags.className, color: '#e5c07b' },
    { tag: tags.definition(tags.variableName), color: '#e06c75' },
    { tag: tags.propertyName, color: '#e06c75' },
    { tag: tags.variableName, color: '#e4e4e7' },
    { tag: tags.attributeName, color: '#d19a66' },
    { tag: tags.tagName, color: '#e06c75' },
    { tag: tags.meta, color: '#abb2bf' },
    { tag: tags.regexp, color: '#98c379' },
    { tag: tags.punctuation, color: '#abb2bf' },
  ]),
  'one-dark': HighlightStyle.define([
    { tag: tags.keyword, color: '#c678dd' },
    { tag: tags.operator, color: '#56b6c2' },
    { tag: tags.string, color: '#98c379' },
    { tag: tags.number, color: '#d19a66' },
    { tag: tags.bool, color: '#d19a66' },
    { tag: tags.null, color: '#d19a66' },
    { tag: tags.comment, color: '#5c6370', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#61afef' },
    { tag: tags.typeName, color: '#e5c07b' },
    { tag: tags.className, color: '#e5c07b' },
    { tag: tags.definition(tags.variableName), color: '#e06c75' },
    { tag: tags.propertyName, color: '#e06c75' },
    { tag: tags.variableName, color: '#abb2bf' },
    { tag: tags.attributeName, color: '#d19a66' },
    { tag: tags.tagName, color: '#e06c75' },
    { tag: tags.meta, color: '#abb2bf' },
    { tag: tags.regexp, color: '#98c379' },
    { tag: tags.punctuation, color: '#abb2bf' },
  ]),
  'dracula': HighlightStyle.define([
    { tag: tags.keyword, color: '#ff79c6' },
    { tag: tags.operator, color: '#ff79c6' },
    { tag: tags.string, color: '#f1fa8c' },
    { tag: tags.number, color: '#bd93f9' },
    { tag: tags.bool, color: '#bd93f9' },
    { tag: tags.null, color: '#bd93f9' },
    { tag: tags.comment, color: '#6272a4', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#50fa7b' },
    { tag: tags.typeName, color: '#8be9fd', fontStyle: 'italic' },
    { tag: tags.className, color: '#8be9fd' },
    { tag: tags.definition(tags.variableName), color: '#f8f8f2' },
    { tag: tags.propertyName, color: '#66d9ef' },
    { tag: tags.variableName, color: '#f8f8f2' },
    { tag: tags.attributeName, color: '#50fa7b' },
    { tag: tags.tagName, color: '#ff79c6' },
    { tag: tags.meta, color: '#f8f8f2' },
    { tag: tags.regexp, color: '#f1fa8c' },
    { tag: tags.punctuation, color: '#f8f8f2' },
  ]),
  'nord': HighlightStyle.define([
    { tag: tags.keyword, color: '#81a1c1' },
    { tag: tags.operator, color: '#81a1c1' },
    { tag: tags.string, color: '#a3be8c' },
    { tag: tags.number, color: '#b48ead' },
    { tag: tags.bool, color: '#81a1c1' },
    { tag: tags.null, color: '#81a1c1' },
    { tag: tags.comment, color: '#616e88', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#88c0d0' },
    { tag: tags.typeName, color: '#8fbcbb' },
    { tag: tags.className, color: '#8fbcbb' },
    { tag: tags.definition(tags.variableName), color: '#d8dee9' },
    { tag: tags.propertyName, color: '#88c0d0' },
    { tag: tags.variableName, color: '#d8dee9' },
    { tag: tags.attributeName, color: '#8fbcbb' },
    { tag: tags.tagName, color: '#81a1c1' },
    { tag: tags.meta, color: '#d8dee9' },
    { tag: tags.regexp, color: '#ebcb8b' },
    { tag: tags.punctuation, color: '#eceff4' },
  ]),
  'github-dark': HighlightStyle.define([
    { tag: tags.keyword, color: '#ff7b72' },
    { tag: tags.operator, color: '#ff7b72' },
    { tag: tags.string, color: '#a5d6ff' },
    { tag: tags.number, color: '#79c0ff' },
    { tag: tags.bool, color: '#79c0ff' },
    { tag: tags.null, color: '#79c0ff' },
    { tag: tags.comment, color: '#8b949e', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#d2a8ff' },
    { tag: tags.typeName, color: '#ffa657' },
    { tag: tags.className, color: '#ffa657' },
    { tag: tags.definition(tags.variableName), color: '#ffa657' },
    { tag: tags.propertyName, color: '#79c0ff' },
    { tag: tags.variableName, color: '#c9d1d9' },
    { tag: tags.attributeName, color: '#79c0ff' },
    { tag: tags.tagName, color: '#7ee787' },
    { tag: tags.meta, color: '#c9d1d9' },
    { tag: tags.regexp, color: '#a5d6ff' },
    { tag: tags.punctuation, color: '#c9d1d9' },
  ]),
  'gruvbox-dark': HighlightStyle.define([
    { tag: tags.keyword, color: '#fb4934' },
    { tag: tags.operator, color: '#8ec07c' },
    { tag: tags.string, color: '#b8bb26' },
    { tag: tags.number, color: '#d3869b' },
    { tag: tags.bool, color: '#d3869b' },
    { tag: tags.null, color: '#d3869b' },
    { tag: tags.comment, color: '#928374', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#b8bb26' },
    { tag: tags.typeName, color: '#fabd2f' },
    { tag: tags.className, color: '#fabd2f' },
    { tag: tags.definition(tags.variableName), color: '#83a598' },
    { tag: tags.propertyName, color: '#83a598' },
    { tag: tags.variableName, color: '#ebdbb2' },
    { tag: tags.attributeName, color: '#fabd2f' },
    { tag: tags.tagName, color: '#8ec07c' },
    { tag: tags.meta, color: '#ebdbb2' },
    { tag: tags.regexp, color: '#b8bb26' },
    { tag: tags.punctuation, color: '#a89984' },
  ]),
  'ayu-mirage': HighlightStyle.define([
    { tag: tags.keyword, color: '#ffad66' },
    { tag: tags.operator, color: '#f29e74' },
    { tag: tags.string, color: '#d5ff80' },
    { tag: tags.number, color: '#dfbfff' },
    { tag: tags.bool, color: '#dfbfff' },
    { tag: tags.null, color: '#dfbfff' },
    { tag: tags.comment, color: '#5c6773', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#ffd173' },
    { tag: tags.typeName, color: '#73d0ff' },
    { tag: tags.className, color: '#73d0ff' },
    { tag: tags.definition(tags.variableName), color: '#73d0ff' },
    { tag: tags.propertyName, color: '#5ccfe6' },
    { tag: tags.variableName, color: '#cccac2' },
    { tag: tags.attributeName, color: '#ffd173' },
    { tag: tags.tagName, color: '#73d0ff' },
    { tag: tags.meta, color: '#cccac2' },
    { tag: tags.regexp, color: '#95e6cb' },
    { tag: tags.punctuation, color: '#b8cfe6' },
  ]),
  'parchment': HighlightStyle.define([
    { tag: tags.keyword, color: '#8b4513' },
    { tag: tags.operator, color: '#6b5344' },
    { tag: tags.string, color: '#2e7d32' },
    { tag: tags.number, color: '#7b1fa2' },
    { tag: tags.bool, color: '#7b1fa2' },
    { tag: tags.null, color: '#7b1fa2' },
    { tag: tags.comment, color: '#9e9688', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#1565c0' },
    { tag: tags.typeName, color: '#c56200' },
    { tag: tags.className, color: '#c56200' },
    { tag: tags.definition(tags.variableName), color: '#ad1457' },
    { tag: tags.propertyName, color: '#1565c0' },
    { tag: tags.variableName, color: '#3b3228' },
    { tag: tags.attributeName, color: '#c56200' },
    { tag: tags.tagName, color: '#ad1457' },
    { tag: tags.meta, color: '#6b5344' },
    { tag: tags.regexp, color: '#2e7d32' },
    { tag: tags.punctuation, color: '#6b5344' },
  ]),
}


export function buildEditorTheme(colors: ThemeColors, isLight = false): Extension {
  return EditorView.theme({
    '&': {
      backgroundColor: colors.bg,
      color: colors.fg,
      height: '100%',
      // The cursor setting draws its shape in this colour.
      '--editor-caret': colors.cursor,
    },
    '.cm-content': {
      caretColor: colors.cursor,
      padding: '8px 0',
    },
    '.cm-cursor, .cm-dropCursor': {
      borderLeftColor: colors.cursor,
      borderLeftWidth: '2px',
    },
    '.cm-selectionBackground, ::selection': {
      backgroundColor: `${colors.selection} !important`,
    },
    '.cm-activeLine': {
      backgroundColor: colors.activeLine,
    },
    '.cm-activeLineGutter': {
      backgroundColor: colors.activeLine,
      color: colors.fg,
    },
    '.cm-gutters': {
      backgroundColor: colors.gutterBg,
      color: colors.gutterFg,
      border: 'none',
      borderRight: `1px solid ${colors.gutterBorder}`,
    },
    '.cm-lineNumbers .cm-gutterElement': {
      padding: '0 8px 0 16px',
      minWidth: '3em',
      fontSize: '11px',
    },
    '.cm-scroller': {
      overflow: 'auto',
      lineHeight: '1.6',
    },
    '.cm-matchingBracket': {
      backgroundColor: colors.bracket,
      outline: `1px solid ${colors.bracketOutline}`,
    },
    '.cm-searchMatch': {
      backgroundColor: colors.searchMatch,
    },
    '.cm-searchMatch.cm-searchMatch-selected': {
      backgroundColor: colors.searchMatchSelected,
    },
    '.cm-selectionMatch': {
      backgroundColor: `${colors.accent}22`,
    },
    '.cm-foldGutter .cm-gutterElement': {
      padding: '0 4px', cursor: 'pointer', color: colors.gutterFg, fontSize: '11px', transition: 'color 0.15s',
    },
    '.cm-foldGutter .cm-gutterElement:hover': { color: colors.fg },
    '.cm-foldPlaceholder': {
      backgroundColor: colors.panelBg, border: `1px solid ${colors.panelBorder}`, color: '#888',
      padding: '0 6px', margin: '0 4px', borderRadius: 'var(--radius-selector)', cursor: 'pointer',
    },
    '.cm-tooltip': {
      backgroundColor: colors.tooltipBg, border: `1px solid ${colors.tooltipBorder}`,
      borderRadius: 'var(--radius-box)', boxShadow: '0 4px 12px rgba(0,0,0,0.5)',
    },
    '.cm-tooltip.cm-tooltip-autocomplete': {
      backgroundColor: colors.tooltipBg, border: `1px solid ${colors.tooltipBorder}`,
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul': {
      fontSize: '11px', maxHeight: '200px',
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul > li': {
      padding: '3px 8px', color: colors.fg,
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]': {
      backgroundColor: `${colors.accent}33`, color: '#fff',
    },
    '.cm-completionMatchedText': { color: colors.accent, textDecoration: 'none' },
    '.cm-panel.cm-search': {
      backgroundColor: colors.panelBg, borderBottom: `1px solid ${colors.panelBorder}`,
      padding: '4px 8px', gap: '4px', fontSize: '11px', color: colors.fg,
    },
    '.cm-panel.cm-search input': {
      backgroundColor: colors.bg, border: `1px solid ${colors.panelBorder}`, color: colors.fg,
      padding: '2px 6px', borderRadius: 'var(--radius-field)', fontSize: '11px', outline: 'none',
    },
    '.cm-panel.cm-search input:focus': { borderColor: colors.accent },
    '.cm-panel.cm-search button': {
      backgroundColor: colors.panelBg, border: `1px solid ${colors.panelBorder}`,
      color: colors.fg, padding: '2px 8px', borderRadius: 'var(--radius-field)', fontSize: '11px', cursor: 'pointer',
    },
    '.cm-panel.cm-search button:hover': { backgroundColor: colors.panelBorder, color: colors.fg },
    '.cm-panel.cm-search label': { color: colors.gutterFg, fontSize: '11px' },
    '.cm-indentation-marker': { opacity: '0.15' },
    '.cm-indentation-marker.active': { opacity: '0.35' },
    '&.cm-focused': { outline: 'none' },
    // Rainbow brackets keep these themes' own colours.
    ...Object.fromEntries(RAINBOW_HEX.map((c, i) => [`.cm-rainbow-${i}`, { color: c }])),
  }, { dark: !isLight })
}

const themeCache = new Map<string, Extension>()

export function isNamedTheme(id: string): boolean {
  return id in THEME_COLORS
}

/** A named theme's editor look (built once per theme). */
export function namedThemeExtension(id: string): Extension {
  let ext = themeCache.get(id)
  if (!ext) {
    ext = buildEditorTheme(THEME_COLORS[id] ?? THEME_COLORS['k2-dark'], LIGHT_THEMES.has(id))
    themeCache.set(id, ext)
  }
  return ext
}

/** A named theme's syntax colours. */
export function namedHighlightStyle(id: string): HighlightStyle {
  return THEME_HIGHLIGHTS[id] ?? THEME_HIGHLIGHTS['k2-dark']
}

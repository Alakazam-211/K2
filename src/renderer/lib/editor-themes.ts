/**
 * Custom theme JSON parser for user-created editor themes.
 * Converts a JSON file from ~/.k2so/themes/ into CodeMirror ThemeColors + HighlightStyle.
 */

import { tags } from '@lezer/highlight'
import { HighlightStyle } from '@codemirror/language'

// ── Types (matches CodeEditor.tsx ThemeColors) ────────────────────────

export interface ThemeColors {
  bg: string
  fg: string
  gutterBg: string
  gutterFg: string
  gutterBorder: string
  activeLine: string
  selection: string
  cursor: string
  bracket: string
  bracketOutline: string
  searchMatch: string
  searchMatchSelected: string
  accent: string
  panelBg: string
  panelBorder: string
  tooltipBg: string
  tooltipBorder: string
}

export interface CustomThemeJson {
  name: string
  type: 'dark' | 'light'
  colors: Partial<ThemeColors> & { bg: string; fg: string }
  syntax: Record<string, string>
}

// ── Defaults (K2 Dark) ──────────────────────────────────────────────

const DEFAULT_COLORS: ThemeColors = {
  bg: '#0a0a0a', fg: '#e4e4e7', gutterBg: '#0a0a0a', gutterFg: '#555', gutterBorder: '#1a1a1a',
  activeLine: '#ffffff08', selection: '#3b82f633', cursor: '#3b82f6', bracket: '#3b82f633',
  bracketOutline: '#3b82f666', searchMatch: '#b5890066', searchMatchSelected: '#b58900aa',
  accent: '#3b82f6', panelBg: '#141414', panelBorder: '#2a2a2a', tooltipBg: '#141414', tooltipBorder: '#2a2a2a',
}

const DEFAULT_SYNTAX: Record<string, string> = {
  keyword: '#c678dd', string: '#98c379', number: '#d19a66', comment: '#5c6370',
  function: '#61afef', type: '#e5c07b', variable: '#e4e4e7', property: '#e06c75',
  operator: '#56b6c2', tag: '#e06c75', attribute: '#d19a66', regexp: '#98c379',
  punctuation: '#abb2bf',
}

// ── Syntax key → Lezer tag mapping ────────────────────────────────────

function buildHighlightStyle(syntax: Record<string, string>): HighlightStyle {
  const s = { ...DEFAULT_SYNTAX, ...syntax }
  return HighlightStyle.define([
    { tag: tags.keyword, color: s.keyword },
    { tag: tags.operator, color: s.operator },
    { tag: tags.string, color: s.string },
    { tag: tags.number, color: s.number },
    { tag: tags.bool, color: s.number },
    { tag: tags.null, color: s.number },
    { tag: tags.comment, color: s.comment, fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: s.function },
    { tag: tags.typeName, color: s.type },
    { tag: tags.className, color: s.type },
    { tag: tags.definition(tags.variableName), color: s.property },
    { tag: tags.propertyName, color: s.property },
    { tag: tags.variableName, color: s.variable },
    { tag: tags.attributeName, color: s.attribute },
    { tag: tags.tagName, color: s.tag },
    { tag: tags.meta, color: s.punctuation },
    { tag: tags.regexp, color: s.regexp },
    { tag: tags.punctuation, color: s.punctuation },
  ])
}

// ── Public API ────────────────────────────────────────────────────────

/**
 * Parse a custom theme JSON string into ThemeColors + HighlightStyle.
 * Returns null if the JSON is invalid (e.g. file mid-edit).
 * Fills missing fields from K2 Dark defaults.
 */
export function parseCustomThemeJson(
  jsonString: string
): { colors: ThemeColors; highlight: HighlightStyle; name: string; type: 'dark' | 'light' } | null {
  try {
    const json = JSON.parse(jsonString) as Partial<CustomThemeJson>
    if (!json.colors || !json.colors.bg || !json.colors.fg) return null

    const colors: ThemeColors = { ...DEFAULT_COLORS, ...json.colors }
    // Auto-derive missing UI colors from bg/fg if not provided
    if (!json.colors.gutterBg) colors.gutterBg = colors.bg
    if (!json.colors.panelBg) colors.panelBg = colors.bg
    if (!json.colors.tooltipBg) colors.tooltipBg = colors.bg

    const highlight = buildHighlightStyle(json.syntax || {})
    const name = json.name || 'Untitled Theme'
    const type = json.type === 'light' ? 'light' : 'dark'

    return { colors, highlight, name, type }
  } catch {
    // Invalid JSON — return null, caller keeps last valid state
    return null
  }
}

/**
 * Serialize a built-in theme's colors and syntax to the JSON format
 * used by custom themes. Used to create templates.
 */
export function serializeThemeToJson(
  name: string,
  type: 'dark' | 'light',
  colors: ThemeColors,
  syntax: Record<string, string>
): string {
  return JSON.stringify({ name, type, colors, syntax }, null, 2)
}

export { DEFAULT_COLORS, DEFAULT_SYNTAX }

// Live docs: which editor a doc tab shows, from what the daemon says.
//
// The daemon decides how a file is served (by name: Markdown first, then
// the text pack's table, then any-file), and says so on `/cli/docs/open`
// and `/cli/docs/status`: `tier` (1 live text, 0 any file), `pack`
// (`markdown` | `text` | `blob`), `family` (`markdown`, the text pack's
// `text` / `code` / `config` / `markup` / `diagram` / `csv-as-text`, or
// `none`) and, for a text file a pack refused, `fallback: {code, message}`.
// This module only turns that answer into an editor choice: no file-name
// rules live here.
//
// The binding contract (k2-mpc-text HOST-INTEGRATION §5): a live text
// editor binds to the `source` root and keeps line breaks as '\n' (no
// custom line separator); the pack writes each '\n' in the file's own
// style and restores its BOM.

/** What the daemon reports for a doc (the fields this module reads). */
export interface LiveDocInfo {
  tier?: number
  pack?: string
  family?: string
  fallback?: { code?: string; message?: string } | null
}

/** The editor a doc tab shows. */
export type LiveDocEditor =
  | 'markdown' // Markdown editor (live)
  | 'plain' // plain text editor (live)
  | 'code' // code editor, highlighted by extension (live)
  | 'csv-text' // a text editor over CSV/TSV, said to be text mode (live)
  | 'any-file' // not live text: presence, lease, versions (tier 0)

/** A preview beside the source, when the family has one. */
export type LiveDocPreview = 'markdown' | 'markup' | 'diagram' | 'grid' | null

export interface LiveDocMode {
  editor: LiveDocEditor
  preview: LiveDocPreview
  /** Live co-editing (tier 1). False: the any-file view (tier 0). */
  live: boolean
  /** The root the editor binds to (both text packs: `source`). */
  root: 'source' | null
  /** Line separator the binding must keep: always '\n' when live. */
  lineSeparator: '\n' | null
  /** Syntax-check config text (JSON, YAML, TOML...) in the code editor. */
  checkSyntax: boolean
  /** A one-line note the tab shows (text mode for CSV; why a file isn't live). */
  note: string | null
}

const SOURCE = 'source' as const

const CSV_NOTE =
  'CSV as text: edits to different rows and cells merge; a sorted or moved row has no identity, and column moves rewrite every line.'

function any(note: string | null): LiveDocMode {
  return {
    editor: 'any-file',
    preview: null,
    live: false,
    root: null,
    lineSeparator: null,
    checkSyntax: false,
    note,
  }
}

function live(editor: LiveDocEditor, preview: LiveDocPreview, extra: Partial<LiveDocMode> = {}): LiveDocMode {
  return {
    editor,
    preview,
    live: true,
    root: SOURCE,
    lineSeparator: '\n',
    checkSyntax: false,
    note: null,
    ...extra,
  }
}

/** The editor mode for a doc, from the daemon's open/status answer. */
export function liveDocMode(info: LiveDocInfo | null | undefined): LiveDocMode {
  if (!info) return any(null)
  const fb = info.fallback
  if (fb && (fb.code || fb.message)) {
    // A text file its pack refused: the any-file view, never a plain editor.
    const why = fb.message || fb.code || ''
    return any(`Not live text: ${why}`)
  }
  if (info.tier !== 1) return any(null)
  switch (info.family) {
    case 'markdown':
      return live('markdown', 'markdown')
    case 'text':
      return live('plain', null)
    case 'code':
      return live('code', null)
    case 'config':
      return live('code', null, { checkSyntax: true })
    case 'markup':
      return live('code', 'markup')
    case 'diagram':
      return live('code', 'diagram')
    case 'csv-as-text':
      return live('csv-text', 'grid', { note: CSV_NOTE })
    default:
      // A family this client doesn't know yet: still live text, plainly.
      return live('plain', null)
  }
}

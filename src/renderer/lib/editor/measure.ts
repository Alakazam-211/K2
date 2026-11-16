// The markdown editor's reading column (Write and Read): the Thread's text
// width (useThreadTextCapPx), so a markdown file reads at the same measure as
// the Thread. Full width is the wider of that and 100 characters. With the
// Thread at Full width (no limit), the column fills the tab. Code files keep
// their own layout and don't read this.

/** The CSS width for `--ed-measure`, from the Thread's limit in px (null: none). */
export function editorMeasure(threadCapPx: number | null, full: boolean): string {
  if (threadCapPx === null) return '100%'
  return full ? `max(${threadCapPx}px, 100ch)` : `${threadCapPx}px`
}

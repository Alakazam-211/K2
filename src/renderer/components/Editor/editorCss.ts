// The editor frame's own styles, scoped under `.ed-tab` (build-editors.md
// E1b; the mock's ed.css). Every colour and radius is a token. One document
// face in both looks (the notes' v3 type): the system sans, 15px in Glass and
// 14.5px in Terminal, with mono kept for code and the chrome. The reading
// column (`--ed-measure`) is the Thread's text width, set on the tab by
// MarkdownEditor (lib/editor/measure.ts). The writing surface is the tile in
// Glass and the page in Terminal.

const SANS = `-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI Variable Text', 'Segoe UI', Roboto, 'Noto Sans', 'Helvetica Neue', Arial, sans-serif`

export const EDITOR_CSS = `
.ed-tab { --ed-body: ${SANS}; --ed-fs: 15px; --ed-lh: 1.62; --ed-h1: 27px; --ed-h2: 20.5px; --ed-h3: 17.5px; }
html[data-style="terminal"] .ed-tab { --ed-fs: 14.5px; --ed-h1: 26px; --ed-h2: 20px; --ed-h3: 17px; }
html[data-style="glass"] .ed-tab { --editor-bg: var(--color-bg-surface); }
.ed-tab .lv-shell { background: var(--editor-bg, var(--color-bg)); }
/* The header row is the selected tab's colour, not the writing surface's: the tab runs into it and its hairline parts it from the text. */
.ed-tab .lv-head { background: var(--color-tab-active); }
.ed-tab .ed-body { background: var(--editor-bg, var(--color-bg)); }
.ed-tab .ed-modes .k2-segment { font-size: 12.5px; }

/* Write and Read are in ./writeModeCss (E2). */

/* The status line: quiet, under the text. */
.ed-tab .ed-status { height: 28px; flex: none; display: flex; align-items: center; gap: 2px; padding: 0 10px; border-top: 1px solid var(--color-border); background: var(--editor-bg, var(--color-bg)); color: var(--color-text-secondary); font-family: var(--font-ui); font-size: 11.5px; white-space: nowrap; position: relative; }
.ed-tab .ed-si { padding: 0 6px; display: inline-flex; align-items: center; gap: 4px; }
.ed-tab .ed-sdot { color: var(--color-text-muted); }
.ed-tab .ed-ok { color: var(--color-status-ok); }
.ed-tab .ed-unsaved-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--color-status-warn); }
.ed-tab .ed-togs { display: inline-flex; align-items: center; gap: 6px; margin-left: 8px; }
.ed-tab .ed-tog { height: 20px; display: inline-flex; align-items: center; gap: 4px; padding: 0 8px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-pill); background: transparent; color: var(--color-text-secondary); font: inherit; cursor: pointer; }
.ed-tab .ed-tog:hover { color: var(--color-text-primary); background: var(--color-control-hover-quiet); }
.ed-tab .ed-tog[aria-pressed="true"] { color: var(--color-text-primary); border-color: var(--color-accent-text); background: color-mix(in srgb, var(--color-accent) 14%, transparent); }
.ed-tab .ed-tog:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }
.ed-tab .ed-facts { display: inline-flex; align-items: center; gap: 2px; margin-left: 8px; padding-left: 8px; border-left: 1px solid var(--color-border); }
.ed-tab .ed-display { position: absolute; right: 10px; bottom: calc(100% + 6px); z-index: 25; display: flex; align-items: center; gap: 6px; padding: 8px; background: var(--color-bg-elevated); border: 1px solid var(--color-border-strong); border-radius: var(--radius-box); box-shadow: var(--shadow-2); }
.ed-tab .ed-display .ed-facts { margin-left: 2px; }
`

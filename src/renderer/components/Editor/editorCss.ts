// The editor frame's own styles, scoped under `.ed-tab` (build-editors.md
// E1b; the mock's ed.css). Every colour and radius is a token. One document
// face in both looks (the notes' v3 type): the system sans, 15px in Glass and
// 14.5px in Terminal, with mono kept for code and the chrome. The reading
// column is 480px (about 70 characters), 600px at Full width. The writing
// surface is the tile in Glass and the page in Terminal.

const SANS = `-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI Variable Text', 'Segoe UI', Roboto, 'Noto Sans', 'Helvetica Neue', Arial, sans-serif`

export const EDITOR_CSS = `
.ed-tab { --ed-body: ${SANS}; --ed-fs: 15px; --ed-lh: 1.62; --ed-measure: 480px; --ed-h1: 27px; --ed-h2: 20.5px; --ed-h3: 17.5px; }
.ed-tab[data-full] { --ed-measure: 600px; }
html[data-style="terminal"] .ed-tab { --ed-fs: 14.5px; --ed-h1: 26px; --ed-h2: 20px; --ed-h3: 17px; }
html[data-style="glass"] .ed-tab { --editor-bg: var(--color-bg-surface); }
.ed-tab .lv-shell, .ed-tab .lv-head { background: var(--editor-bg, var(--color-bg)); }
.ed-tab .ed-body { background: var(--editor-bg, var(--color-bg)); }
.ed-tab .ed-modes .k2-segment { font-size: 12.5px; }

/* Write (E1b): CodeMirror in the document face at the reading measure, no
   gutters. TODO(E2): Write becomes the live-preview decorations. */
.ed-tab[data-mode="write"] .cm-editor { font-family: var(--ed-body); font-size: var(--ed-fs); }
.ed-tab[data-mode="write"] .cm-editor .cm-scroller { font-family: var(--ed-body); line-height: var(--ed-lh); padding: 0 24px; }
.ed-tab[data-mode="write"] .cm-editor .cm-content { flex: 0 1 auto; width: var(--ed-measure); max-width: 100%; margin: 0 auto; padding: 28px 0 40px; }
.ed-tab[data-mode="write"] .cm-editor .cm-gutters { display: none; }

/* Read (E1b): today's renderer on the buffer, restyled to the measure. */
.ed-tab .ed-read-scroll { flex: 1; min-height: 0; overflow-y: auto; overflow-x: hidden; padding: 0 24px; }
.ed-tab .ed-read-scroll:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: -2px; }
html[data-style="glass"] .ed-tab .ed-read-scroll { border-radius: var(--radius-box); }
.ed-tab .ed-read { max-width: var(--ed-measure); margin: 0 auto; padding: 28px 0 40px; font-family: var(--ed-body); font-size: var(--ed-fs); line-height: var(--ed-lh); color: var(--color-text-primary); user-select: text; -webkit-user-select: text; }
.ed-tab .ed-read :is(p, ul, ol, li, td) { color: var(--color-text-primary); }
.ed-tab .ed-read :is(h1, h2, h3, h4) { font-family: var(--ed-body); font-weight: 700; color: var(--color-text-primary); border-bottom: 0; padding-bottom: 0; line-height: 1.3; }
.ed-tab .ed-read h1 { font-size: var(--ed-h1); margin: 0 0 16px; }
.ed-tab .ed-read h2 { font-size: var(--ed-h2); margin: 32px 0 12px; }
.ed-tab .ed-read h3 { font-size: var(--ed-h3); margin: 24px 0 8px; }
.ed-tab .ed-read code { font-family: var(--font-code); }
.ed-tab .ed-read blockquote { background: none; border-left: 3px solid var(--color-border-strong); color: var(--color-text-secondary); padding: 2px 0 2px 16px; }
.ed-tab .ed-read blockquote p { color: var(--color-text-secondary); }
.ed-tab .ed-read table { font-size: 0.9em; }
.ed-tab .ed-read :is(thead th, tbody td) { border: 0; border-bottom: 1px solid var(--color-border-strong); background: none; }
.ed-tab .ed-read thead th { border-bottom: 1.5px solid var(--color-control-edge); }
.ed-tab .ed-read img { border: 1px solid var(--color-border); border-radius: var(--radius-box); }

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

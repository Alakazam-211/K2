// Write and Read's page (build-editors.md E2; the mock's ed.css, v3 type and
// the v3.1 table rule). Scoped to the editor frame and to CodeMirror's
// `.cm-md` (lib/editor/write-mode). Every colour, radius and cut is a token.
// Marks shown on the caret's line use the muted ink. List, task and quote
// markers hang left of the text (--tx), so revealing them never moves a word.

export const WRITE_CSS = `
.ed-tab .cm-editor.cm-md { font-family: var(--ed-body); font-size: var(--ed-fs); }
.ed-tab .cm-md .cm-scroller { font-family: var(--ed-body); line-height: var(--ed-lh); padding: 0 24px; }
.ed-tab .cm-md .cm-content { flex: 0 1 auto; width: var(--ed-measure); max-width: 100%; margin: 0 auto; padding: 28px 0 40px; }
.ed-tab .cm-md .cm-gutters { display: none; }
.ed-tab .cm-md .cm-line { padding: 0 2px 0 0; }
.ed-tab .cm-md .cm-activeLine { background: transparent; }
.ed-tab .cm-md [class*="cm-rainbow-"] { color: inherit; }
/* Code-only aids stay out of the page: changed-line tints and indent guides. The
   tints are !important in a layer (globals.css), so the page's own ground is painted
   over them as an inset shadow (above a background, below the text). */
.ed-tab .cm-md .cm-line:is(.cm-git-line-added, .cm-git-line-modified, .cm-diff-line-added, .cm-diff-line-modified, .cm-diff-line-deleted) { box-shadow: inset 0 0 0 100vmax var(--editor-bg, var(--color-bg)); border-left: 0; }
.ed-tab .cm-md .cm-line.cm-md-fence:is(.cm-git-line-added, .cm-git-line-modified, .cm-diff-line-added, .cm-diff-line-modified, .cm-diff-line-deleted) { box-shadow: inset 0 0 0 100vmax var(--color-bg-inset); border-left: 1px solid var(--color-border); }
.ed-tab .cm-md .cm-indent-markers:not(.cm-md-quote)::before { display: none; }
.ed-tab .cm-md-read .cm-cursorLayer { display: none; }
.ed-tab .cm-md-read .cm-content { caret-color: transparent; }
.ed-tab .cm-editor.cm-md-read:has(.cm-content:focus-visible) { outline: 2px solid var(--color-accent-text); outline-offset: -2px; border-radius: var(--radius-box); }

.ed-tab .cm-md .cm-md-mark { color: var(--color-text-muted); font-weight: 400; font-style: normal; text-decoration: none; }

.ed-tab .cm-md .cm-line.cm-md-h1 { font-size: var(--ed-h1); font-weight: 700; line-height: 1.25; letter-spacing: -0.01em; padding-top: 4px; padding-bottom: 10px; }
.ed-tab .cm-md .cm-line.cm-md-h2 { font-size: var(--ed-h2); font-weight: 700; line-height: 1.3; padding-top: 22px; padding-bottom: 6px; }
.ed-tab .cm-md .cm-line.cm-md-h3 { font-size: var(--ed-h3); font-weight: 700; line-height: 1.35; padding-top: 16px; padding-bottom: 4px; }
.ed-tab .cm-md .cm-line.cm-md-h4 { font-weight: 700; padding-top: 12px; }
.ed-tab .cm-md .cm-line.cm-md-setext-rule { line-height: 1; }
.ed-tab .cm-md .cm-line.cm-md-blank { font-size: 0.45em; line-height: 1.66; }

.ed-tab .cm-md .cm-md-strong { font-weight: 680; }
.ed-tab .cm-md .cm-md-em { font-style: italic; }
.ed-tab .cm-md .cm-md-strike { text-decoration: line-through; }
.ed-tab .cm-md .cm-md-code { font-family: var(--font-code); font-size: 0.87em; color: var(--color-code-inline); background: var(--color-bg-inset); padding: 1px 4px; border-radius: var(--radius-badge); }
.ed-tab .cm-md .cm-md-link { color: var(--color-accent-text); text-decoration: underline; text-underline-offset: 2px; text-decoration-thickness: 1px; }
.ed-tab .cm-md .cm-md-link:hover { text-decoration-thickness: 2px; }
.ed-tab .cm-md-read .cm-md-link { cursor: pointer; }
.ed-tab .cm-md .cm-md-caption { font-size: 12.5px; color: var(--color-text-muted); display: inline-block; }

/* Markers hang left of the text, revealed or not. */
.ed-tab .cm-md .cm-line:is(.cm-md-li, .cm-md-quote) { position: relative; padding-left: var(--tx); }
.ed-tab .cm-md .cm-line.cm-md-li { --tx: calc(22px + var(--ind, 0) * 22px); }
.ed-tab .cm-md .cm-line.cm-md-task { --tx: calc(30px + var(--ind, 0) * 22px); }
.ed-tab .cm-md .cm-line.cm-md-quote { --tx: 16px; }
.ed-tab .cm-md .cm-md-lm { position: absolute; right: calc(100% - var(--tx)); white-space: pre; }
.ed-tab .cm-md .cm-md-num { color: var(--color-text-secondary); font-variant-numeric: tabular-nums; }
.ed-tab .cm-md .cm-md-bullet { position: absolute; left: calc(6px + var(--ind, 0) * 22px); top: calc(var(--ed-fs) * var(--ed-lh) / 2 - 3px); width: 6px; height: 6px; border-radius: 50%; background: var(--color-text-secondary); }
.ed-tab .cm-md .cm-md-bullet[data-shape="ring"] { background: transparent; box-shadow: inset 0 0 0 1.5px var(--color-text-secondary); }
html[data-style="terminal"] .ed-tab .cm-md .cm-md-bullet { border-radius: 0; }
.ed-tab .cm-md .cm-md-check { position: absolute; left: calc(4px + var(--ind, 0) * 22px); top: calc(var(--ed-fs) * var(--ed-lh) / 2 - var(--control-check) / 2); display: inline-flex; line-height: 0; }
.ed-tab .cm-md .cm-md-check .k2-checkbox { margin: 0; }
.ed-tab .cm-md-read .cm-md-check .k2-checkbox { cursor: default; }
.ed-tab .cm-md .cm-md-done { color: var(--color-text-secondary); text-decoration: line-through; text-decoration-color: color-mix(in srgb, var(--color-text-secondary) 70%, transparent); }
.ed-tab .cm-md .cm-line.cm-md-quote::before { content: ''; position: absolute; left: 0; top: 3px; bottom: 3px; width: 3px; background: var(--color-border-strong); border-radius: var(--radius-badge); }
html[data-style="terminal"] .ed-tab .cm-md .cm-line.cm-md-quote::before { border-radius: 0; }
.ed-tab .cm-md .cm-line.cm-md-quote:has(.cm-md-lm .cm-md-mark)::before { display: none; }
.ed-tab .cm-md .cm-md-quote-text { color: var(--color-text-secondary); }

/* A rule: the text hides, a hairline draws through the line. */
.ed-tab .cm-md .cm-line.cm-md-hr { position: relative; }
.ed-tab .cm-md .cm-line.cm-md-hr:has(.cm-md-mark)::after { display: none; }
.ed-tab .cm-md .cm-line.cm-md-hr::after { content: ''; position: absolute; left: 0; right: 0; top: 50%; height: 1px; background: var(--color-border-strong); pointer-events: none; }

/* Code fences: one card in the code well, a header line with the language and Copy.
   Terminal cuts the card's corners (globals.css, data-k2-chamfer-rows). */
.ed-tab .cm-md .cm-line.cm-md-fence { --k2-ch-edge: var(--color-border); font-family: var(--font-code); font-size: 13px; line-height: 1.6; background-color: var(--color-bg-inset); padding: 0 14px; border-left: 1px solid var(--color-border); border-right: 1px solid var(--color-border); }
html[data-style="terminal"] .ed-tab .cm-md .cm-line.cm-md-fence { font-size: 12.5px; }
.ed-tab .cm-md .cm-line.cm-md-fence-top { border-top: 1px solid var(--color-border); border-top-left-radius: var(--radius-box); border-top-right-radius: var(--radius-box); padding-top: 10px; }
.ed-tab .cm-md .cm-line.cm-md-fence-end { border-bottom: 1px solid var(--color-border); border-bottom-left-radius: var(--radius-box); border-bottom-right-radius: var(--radius-box); padding-bottom: 12px; }
.ed-tab .cm-md .cm-line.cm-md-fence-open { padding-bottom: 6px; }
.ed-tab .cm-md .cm-line.cm-md-fence-close { line-height: 1; }
html[data-style="terminal"] .ed-tab .cm-md .cm-line.cm-md-fence { border-radius: 0; }
.ed-tab .cm-md .cm-md-plaincode { color: var(--color-text-primary); }
.ed-tab .cm-md .cm-md-fence-head { display: inline-flex; align-items: center; width: 100%; vertical-align: top; font-family: var(--ed-body); font-size: 11.5px; line-height: 26px; color: var(--color-text-secondary); }
.ed-tab .cm-md .cm-md-fence-lang { flex: 1; }
.ed-tab .cm-md .cm-md-copy { width: 26px; height: 26px; margin-right: -6px; display: inline-grid; place-items: center; padding: 0; border: 0; border-radius: var(--radius-field); background: transparent; color: var(--color-text-secondary); cursor: pointer; }
.ed-tab .cm-md .cm-md-copy:hover { color: var(--color-text-primary); background: var(--color-control-hover-quiet); }
.ed-tab .cm-md .cm-md-copy[data-copied] { color: var(--color-status-ok); }
.ed-tab .cm-md .cm-md-copy:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 1px; }

/* Tables: a header rule and a hairline under every row, as the CSV table. */
.ed-tab .cm-md .cm-md-table-wrap { padding: 8px 0 14px; overflow-x: auto; }
.ed-tab .cm-md .cm-md-table { width: 100%; border-collapse: collapse; font-size: 0.93em; line-height: 1.45; }
.ed-tab .cm-md .cm-md-table th { text-align: left; font-weight: 650; color: var(--color-text-primary); border-bottom: 1.5px solid var(--color-control-edge); padding: 6px 10px 6px 0; }
html[data-style="terminal"] .ed-tab .cm-md .cm-md-table th { font-family: var(--font-code); font-size: 12.5px; }
.ed-tab .cm-md .cm-md-table td { padding: 6px 10px 6px 0; border-bottom: 1px solid var(--color-border-strong); vertical-align: top; color: var(--color-text-primary); }
.ed-tab .cm-md .cm-line.cm-md-table-src { font-family: var(--font-code); font-size: 13px; white-space: pre; }

/* Images resolve against the file; the line under the picture is its caption. */
.ed-tab .cm-md .cm-md-figure { padding: 10px 0 6px; }
.ed-tab .cm-md .cm-md-figure img { display: block; max-width: 100%; height: auto; border-radius: var(--radius-box); box-shadow: 0 0 0 1px var(--color-border); }
html[data-style="terminal"] .ed-tab .cm-md .cm-md-figure img { border-radius: 0; }
.ed-tab .cm-md .cm-md-figure[data-missing] { font-size: 12.5px; color: var(--color-text-muted); border: 1px dashed var(--color-border-strong); border-radius: var(--radius-box); padding: 14px; margin: 10px 0 6px; }
`

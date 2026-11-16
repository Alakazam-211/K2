// E3's tools in the editor frame (build-editors.md E3; the mock's ed.css
// `.find`, `.fmt`, `.slash`, the outline's `.orow` and the status line's
// `.sb`). Scoped under `.ed-tab`; every colour, radius and cut is a token.
// Glass: round, the frost float for the toolbar, the / menu and go to line.
// Terminal: square, cut where the family cuts (data-k2-chamfer), the page fill
// with the edge for the floats.

export const TOOLS_CSS = `
/* CodeMirror's own search panel never shows: the find bar drives its search. */
.ed-tab .cm-panels:has(.ed-find-proxy) { display: none; }
.ed-tab .cm-searchMatch { border-radius: 2px; }
.ed-tab .cm-searchMatch-selected { box-shadow: 0 0 0 2px var(--color-status-warn); }
.ed-tab .cm-searchMatch, .ed-tab .cm-searchMatch * { color: var(--color-text-primary); }

/* The find bar: docked under the file bar, full width, pushing the text down. */
.ed-tab .ed-find { flex: none; display: flex; flex-direction: column; gap: 6px; padding: 6px 10px 6px 12px; border-bottom: 1px solid var(--color-border); background: var(--editor-bg, var(--color-bg)); }
html[data-style="terminal"] .ed-tab .ed-find { background: var(--color-bg-surface); }
.ed-tab .ed-frow { display: flex; align-items: center; gap: 4px; min-width: 0; }
.ed-tab .ed-fic { width: 18px; flex: none; display: grid; place-items: center; color: var(--color-text-secondary); }
.ed-tab .ed-fld { flex: 0 1 360px; min-width: 80px; height: 28px; padding: 0 10px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-field); background: var(--color-bg); color: var(--color-text-primary); font: 12.5px var(--font-ui); }
html[data-style="glass"] .ed-tab .ed-fld { background: var(--color-bg-elevated); }
.ed-tab .ed-fld::placeholder { color: var(--color-text-muted); }
.ed-tab .ed-fld:hover { border-color: var(--color-text-secondary); }
.ed-tab .ed-fld:focus { border-color: var(--color-accent-text); }
.ed-tab .ed-fld:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }
html[data-style="terminal"] .ed-tab .ed-fld:focus-visible { outline: none; box-shadow: inset 0 0 0 1px var(--color-accent-text); }
.ed-tab .ed-fcount { flex: none; min-width: 72px; padding: 0 6px; text-align: center; white-space: nowrap; font-size: 12px; color: var(--color-text-secondary); font-variant-numeric: tabular-nums; }
.ed-tab .ed-fcount-none { color: var(--color-text-primary); }
.ed-tab .ed-find-btn { width: 28px; height: 28px; flex: none; display: inline-flex; align-items: center; justify-content: center; border: 1px solid transparent; border-radius: var(--radius-field); background: transparent; color: var(--color-text-secondary); cursor: pointer; }
.ed-tab .ed-find-btn[aria-disabled="true"] { color: var(--color-text-muted); cursor: default; }
.ed-tab .ed-find-btn[aria-disabled="true"]:hover { background: transparent; color: var(--color-text-muted); }
.ed-tab .ed-find-btn[aria-pressed="true"] { border-color: var(--color-accent-text); background: color-mix(in srgb, var(--color-accent) 20%, transparent); color: var(--color-text-primary); }
.ed-tab .ed-find-tgl { width: 28px; height: 28px; flex: none; display: grid; place-items: center; padding: 0; border: 1px solid transparent; border-radius: var(--radius-field); background: transparent; color: var(--color-text-secondary); font: 600 12px/1 var(--font-ui); cursor: pointer; }
html[data-style="terminal"] .ed-tab .ed-find-tgl { font-family: var(--font-code); }
.ed-tab .ed-find-tgl:hover { background: var(--color-control-hover-quiet); color: var(--color-text-primary); }
.ed-tab .ed-find-tgl[aria-pressed="true"] { border-color: var(--color-accent-text); background: color-mix(in srgb, var(--color-accent) 20%, transparent); color: var(--color-text-primary); }
.ed-tab .ed-find-act { height: 28px; flex: none; padding: 0 12px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-field); background: var(--color-bg-elevated); color: var(--color-text-primary); font: 12.5px var(--font-ui); white-space: nowrap; cursor: pointer; }
.ed-tab .ed-find-act:hover { background: var(--color-control-hover); }
.ed-tab .ed-find-act[aria-disabled="true"] { color: var(--color-text-muted); background: transparent; cursor: default; }
.ed-tab :is(.ed-find-btn, .ed-find-tgl, .ed-find-act, .ed-fb, .ed-sb):focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }
html[data-style="terminal"] .ed-tab :is(.ed-find-btn, .ed-find-tgl, .ed-find-act, .ed-fb, .ed-sb):focus-visible { outline: none; box-shadow: inset 0 0 0 2px var(--color-accent-text); }

/* The formatting toolbar: in the right margin beside the selection; docked in the file bar when there's no margin. */
.ed-tab .ed-fmt { z-index: 20; display: flex; align-items: center; gap: 2px; padding: 4px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-box); background: var(--color-bg-elevated); box-shadow: var(--shadow-2); }
html[data-style="terminal"] .ed-tab .ed-fmt { background: var(--color-bg); border-color: var(--color-border-strong); }
.ed-tab .ed-fmt[data-docked] { position: static; flex: none; padding: 2px; box-shadow: none; }
html[data-style="terminal"] .ed-tab .ed-fmt[data-docked] { background: var(--color-bg-surface); }
.ed-tab .ed-fb { width: 30px; height: 30px; display: grid; place-items: center; padding: 0; border: 1px solid transparent; border-radius: var(--radius-field); background: transparent; color: var(--color-text-primary); cursor: pointer; }
.ed-tab .ed-fmt[data-docked] .ed-fb { width: 28px; height: 28px; }
.ed-tab .ed-fb:hover { background: var(--color-control-hover-quiet); }
.ed-tab .ed-fb[aria-pressed="true"] { background: color-mix(in srgb, var(--color-accent) 22%, transparent); border-color: color-mix(in srgb, var(--color-accent-text) 60%, transparent); }
.ed-tab .ed-fsep { width: 1px; height: 18px; margin: 0 3px; background: var(--color-border-strong); }

/* The / menu: under the "/" line. */
.ed-tab .ed-slash { z-index: 21; width: 280px; padding: 6px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-box); background: var(--color-bg-elevated); box-shadow: var(--shadow-2); font: 12.5px/1.3 var(--font-ui); }
html[data-style="terminal"] .ed-tab .ed-slash { background: var(--color-bg); border-color: var(--color-border-strong); }
.ed-tab .ed-slash-head { margin: 0; padding: 4px 8px 6px; font-size: 11.5px; color: var(--color-text-secondary); }
.ed-tab .ed-slash-opt { display: flex; align-items: center; gap: 10px; padding: 6px 8px; border-radius: calc(var(--radius-box) - 6px); color: var(--color-text-primary); cursor: pointer; }
html[data-style="terminal"] .ed-tab .ed-slash-opt { border-radius: 0; }
.ed-tab .ed-slash-opt:hover, .ed-tab .ed-slash-on { background: var(--color-row-hover); }
.ed-tab .ed-slash-on { box-shadow: inset 3px 0 0 var(--color-accent-text); }
.ed-tab .ed-slash-tile { width: 26px; height: 26px; flex: none; display: grid; place-items: center; border: 1px solid var(--color-border-strong); border-radius: var(--radius-field); background: var(--color-bg-surface); color: var(--color-text-secondary); }
.ed-tab .ed-slash-name { flex: 1; min-width: 0; }
.ed-tab .ed-slash-hint { font: 500 11px/1 var(--font-code); color: var(--color-text-secondary); }

/* Go to line: a small field over the status line's left end. */
.ed-tab .ed-goto { position: absolute; left: 8px; bottom: calc(100% + 6px); z-index: 25; width: 240px; padding: 8px 10px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-box); background: var(--color-bg-elevated); box-shadow: var(--shadow-2); }
html[data-style="terminal"] .ed-tab .ed-goto { background: var(--color-bg); border-color: var(--color-border-strong); }
.ed-tab .ed-goto-row { display: flex; align-items: center; gap: 8px; }
.ed-tab .ed-goto-label { flex: none; font: 600 12px var(--font-ui); color: var(--color-text-primary); }
.ed-tab .ed-goto-fld { flex: 1; width: 0; }
.ed-tab .ed-goto-note { margin: 6px 0 0; font-size: 11.5px; color: var(--color-text-secondary); }
.ed-tab .ed-goto-bad { color: var(--color-status-error-text); }

/* Ln, Col: a quiet button. */
.ed-tab .ed-sb { height: 22px; display: inline-flex; align-items: center; padding: 0 7px; margin: 0 -7px; border: 1px solid transparent; border-radius: var(--radius-field); background: transparent; color: inherit; font: inherit; cursor: pointer; }
.ed-tab .ed-sb:hover { background: var(--color-control-hover-quiet); color: var(--color-text-primary); }

/* The side panel's tabs and the outline's rows (the family's hover band and bar). */
.ed-tab .ed-panel-tab { background: transparent; font: inherit; }
.ed-tab .ed-panel-tab[aria-selected="true"] { background: var(--color-bg-elevated); }
.ed-tab .ed-panel-tab-off:hover { background: var(--color-control-hover-quiet); color: var(--color-text-primary); }
.ed-tab .ed-panel-tab:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }
.ed-tab .ed-orow { display: flex; width: calc(100% - 8px); margin: 0 4px; align-items: center; gap: 8px; padding: 5px 12px 5px calc(10px + var(--oi, 0) * 14px); text-align: left; color: var(--color-text-primary); cursor: pointer; border-radius: var(--radius-field); }
html[data-style="terminal"] .ed-tab .ed-orow { border-radius: 0; }
.ed-tab .ed-orow-top { font-weight: 650; }
.ed-tab .ed-orow:hover { background: var(--color-row-hover); box-shadow: inset 3px 0 0 var(--color-text-secondary); }
.ed-tab .ed-orow[aria-current] { background: var(--color-row-hover); box-shadow: inset 3px 0 0 var(--color-accent-text); }
.ed-tab .ed-orow:focus-visible { outline: none; box-shadow: inset 0 0 0 2px var(--color-accent-text), inset 4px 0 0 var(--color-accent-text); }
`

// The PDF tab's own styles, scoped under `.pdf-tab` (the editors' frame and
// status line come from EDITOR_CSS). Every colour, radius and shadow is a
// token; the page itself is white paper (pdf.js fills the canvas), on the
// desk's inset ground. pdf.js's text and annotation layers need a few of
// its own layout rules (from pdfjs-dist/web/pdf_viewer.css, Apache-2.0),
// kept here without its colours. Find's marks use the editors' find tokens.

export const PDF_CSS = `
.pdf-tab .pdf-head-ctrls { display: flex; align-items: center; gap: 2px; }
.pdf-tab .pdf-head-btn { width: 30px; height: 30px; display: inline-flex; align-items: center; justify-content: center; border: 0; border-radius: var(--radius-field); background: transparent; color: var(--color-text-secondary); cursor: pointer; }
.pdf-tab .pdf-head-btn:disabled { opacity: .4; cursor: default; background: transparent; }
.pdf-tab .pdf-zoom-btn { height: 30px; min-width: 50px; padding: 0 6px; border: 0; border-radius: var(--radius-field); background: transparent; color: var(--color-text-primary); cursor: pointer; display: inline-flex; align-items: center; justify-content: center; }
.pdf-tab .pdf-page-of { min-width: 48px; text-align: center; color: var(--color-text-primary); white-space: nowrap; }
.pdf-tab .pdf-head-sep { width: 1px; height: 16px; margin: 0 6px; background: var(--color-border); flex: none; }
.pdf-tab :is(.pdf-head-btn, .pdf-zoom-btn, .pdf-find-btn, .pdf-chip, .pdf-panel-tab, .pdf-thumb, .pdf-annot-row, .pdf-group-btn, .pdf-pop-close):focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }

/* The desk and the paper. */
.pdf-tab .pdf-desk { flex: 1 1 auto; min-height: 0; overflow: auto; background: var(--color-bg-inset); }
.pdf-tab .pdf-desk-inner { display: flex; justify-content: center; align-items: flex-start; box-sizing: border-box; min-width: fit-content; min-height: 100%; padding: 24px; }
.pdf-tab .pdf-sheet { position: relative; flex: none; box-shadow: var(--shadow-2); --scale-round-x: 1px; --scale-round-y: 1px; }
.pdf-tab .pdf-canvas { display: block; }

/* pdf.js's text layer: transparent text over the drawing, selectable. */
.pdf-tab .textLayer { position: absolute; inset: 0; overflow: clip; opacity: 1; line-height: 1; text-align: initial; text-size-adjust: none; -webkit-text-size-adjust: none; forced-color-adjust: none; transform-origin: 0 0; caret-color: CanvasText; z-index: 0; color-scheme: only light;
  --min-font-size: 1; --text-scale-factor: calc(var(--total-scale-factor) * var(--min-font-size)); --min-font-size-inv: calc(1 / var(--min-font-size)); }
.pdf-tab .textLayer :is(span, br) { color: transparent; position: absolute; white-space: pre; cursor: text; transform-origin: 0% 0%; }
.pdf-tab .textLayer > :not(.markedContent), .pdf-tab .textLayer .markedContent span:not(.markedContent) { z-index: 1; --font-height: 0; font-size: calc(var(--text-scale-factor) * var(--font-height)); --scale-x: 1; --rotate: 0deg; transform: rotate(var(--rotate)) scaleX(var(--scale-x)) scale(var(--min-font-size-inv)); }
.pdf-tab .textLayer .markedContent { display: contents; }
.pdf-tab .textLayer span[role="img"] { user-select: none; -webkit-user-select: none; cursor: default; }
.pdf-tab .textLayer ::selection { background: color-mix(in srgb, var(--color-accent) 30%, transparent); }
.pdf-tab .textLayer br::selection { background: transparent; }
.pdf-tab .textLayer .endOfContent { display: block; position: absolute; inset: 100% 0 0; z-index: 0; cursor: default; user-select: none; -webkit-user-select: none; }
.pdf-tab .textLayer.selecting .endOfContent { top: 0; }
.pdf-tab .textLayer .pdf-hl { position: static; margin: -1px; padding: 1px; border-radius: 2px; background: var(--color-find-match); }
.pdf-tab .textLayer .pdf-hl[data-current] { background: var(--color-find-match-current); box-shadow: 0 0 0 2px var(--color-status-warn); }

/* pdf.js's annotation layer: links and a hit area per annotation; its own note popups stay hidden (the popover is K2's). */
.pdf-tab .annotationLayer { position: absolute; top: 0; left: 0; pointer-events: none; transform-origin: 0 0; color-scheme: only light; }
.pdf-tab .annotationLayer section { position: absolute; text-align: initial; pointer-events: auto; box-sizing: border-box; transform-origin: 0 0; user-select: none; -webkit-user-select: none; }
.pdf-tab .textLayer.selecting ~ .annotationLayer section { pointer-events: none; }
.pdf-tab .annotationLayer .annotationTextContent { position: absolute; inset: 0; opacity: 0; color: transparent; pointer-events: none; user-select: none; -webkit-user-select: none; }
.pdf-tab .annotationLayer .annotationTextContent span { position: relative; }
.pdf-tab .annotationLayer .annotationContent { position: absolute; width: 100%; height: 100%; pointer-events: none; }
.pdf-tab .annotationLayer :is(.popupAnnotation, .popup) { display: none !important; }
.pdf-tab .annotationLayer .textAnnotation img { position: absolute; inset: 0; width: 100%; height: 100%; opacity: 0; cursor: pointer; }
.pdf-tab .annotationLayer .linkAnnotation > a { position: absolute; top: 0; left: 0; width: 100%; height: 100%; font-size: 1em; }
.pdf-tab .annotationLayer .linkAnnotation > a:hover { background: color-mix(in srgb, var(--color-accent) 18%, transparent); }
.pdf-tab .annotationLayer .linkAnnotation > a:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 1px; }
.pdf-tab .annotationLayer section[data-pdf-annot] { cursor: pointer; }
.pdf-tab .annotationLayer section[data-pdf-annot]:hover { background: color-mix(in srgb, var(--color-accent) 10%, transparent); }
.pdf-tab .annotationLayer section[data-selected] { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }
.pdf-tab .annotationLayer section[data-pdf-note] { background: var(--color-status-warn); border: 1px solid var(--color-status-warn-text); }

/* The annotation popover. */
.pdf-tab .pdf-pop { position: absolute; z-index: 30; box-sizing: border-box; padding: 10px 12px 12px; background: var(--color-bg-elevated); border: 1px solid var(--color-border-strong); border-radius: var(--radius-box); box-shadow: var(--shadow-2); color: var(--color-text-primary); font-family: var(--font-ui); text-align: left; }
.pdf-tab .pdf-pop:focus { outline: none; }
.pdf-tab .pdf-pop:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }
.pdf-tab .pdf-pop-head { display: flex; align-items: center; gap: 6px; min-width: 0; }
.pdf-tab .pdf-pop-head b { overflow-wrap: anywhere; }
.pdf-tab .pdf-pop-close { width: 24px; height: 24px; flex: none; display: inline-flex; align-items: center; justify-content: center; border: 0; border-radius: var(--radius-field); background: transparent; color: var(--color-text-secondary); cursor: pointer; }
.pdf-tab .pdf-pop-meta { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 2px; color: var(--color-text-secondary); }
.pdf-tab .pdf-pop-text { margin: 8px 0 0; white-space: pre-wrap; overflow-wrap: anywhere; max-height: 220px; overflow: auto; }

/* Find, docked under the header (the editors' find bar). */
.pdf-tab .pdf-find { flex: none; display: flex; align-items: center; gap: 4px; padding: 6px 10px 6px 12px; border-bottom: 1px solid var(--color-border); background: var(--editor-bg, var(--color-bg)); }
html[data-style="terminal"] .pdf-tab .pdf-find { background: var(--color-bg-stripe); }
.pdf-tab .pdf-find-icon { width: 18px; flex: none; display: grid; place-items: center; color: var(--color-text-secondary); }
.pdf-tab .pdf-find-field { flex: 0 1 360px; min-width: 0; height: 28px; box-sizing: border-box; padding: 0 10px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-field); background: var(--color-bg); color: var(--color-text-primary); font: inherit; font-size: 12.5px; }
html[data-style="glass"] .pdf-tab .pdf-find-field { background: var(--color-bg-elevated); }
.pdf-tab .pdf-find-field::placeholder { color: var(--color-text-secondary); }
.pdf-tab .pdf-find-field:hover { border-color: var(--color-text-secondary); }
.pdf-tab .pdf-find-field:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 1px; }
.pdf-tab .pdf-find-count { min-width: 72px; padding: 0 6px; text-align: center; white-space: nowrap; font-size: 12px; color: var(--color-text-secondary); }
.pdf-tab .pdf-find-count[data-none] { color: var(--color-text-primary); }
.pdf-tab .pdf-find-btn { width: 28px; height: 28px; flex: none; display: inline-flex; align-items: center; justify-content: center; border: 0; border-radius: var(--radius-field); background: transparent; color: var(--color-text-secondary); cursor: pointer; }
.pdf-tab .pdf-find-btn[aria-disabled] { opacity: .45; cursor: default; background: transparent; }
.pdf-tab .pdf-find-case { width: 32px; font: 600 12px var(--font-ui); }
.pdf-tab .pdf-find-case[aria-pressed="true"] { color: var(--color-text-primary); background: color-mix(in srgb, var(--color-accent) 14%, transparent); box-shadow: inset 0 0 0 1px var(--color-accent-text); }

/* The side panel. */
.pdf-tab .pdf-panel-tab { display: inline-flex; align-items: center; gap: 5px; padding: 3px 8px; font-size: 12.5px; white-space: nowrap; border: 1px solid transparent; border-radius: var(--radius-pill); background: transparent; color: var(--color-text-secondary); font-family: inherit; cursor: pointer; }
.pdf-tab .pdf-panel-tab:hover { color: var(--color-text-primary); background: var(--color-control-hover-quiet); }
.pdf-tab .pdf-panel-tab[aria-selected="true"] { border-color: var(--color-control-edge); color: var(--color-text-primary); }
.pdf-tab .pdf-tab-count, .pdf-tab .pdf-chip-count, .pdf-tab .pdf-quiet, .pdf-tab .pdf-kind { color: var(--color-text-secondary); }
.pdf-tab .pdf-dot { color: var(--color-text-secondary); }
.pdf-tab .pdf-empty { padding: 12px 14px; color: var(--color-text-secondary); }
.pdf-tab .pdf-list-head { display: flex; align-items: center; gap: 6px; padding: 8px 12px 6px 14px; color: var(--color-text-secondary); }
.pdf-tab .pdf-chip { padding: 2px 10px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-pill); color: var(--color-text-primary); cursor: pointer; }
.pdf-tab .pdf-chip[aria-checked="true"] { border-color: var(--color-accent-text); background: color-mix(in srgb, var(--color-accent) 14%, transparent); }
.pdf-tab .pdf-checking { padding: 0 14px 6px; color: var(--color-text-secondary); }
.pdf-tab .pdf-layer-row { display: flex; align-items: center; gap: 10px; min-height: 34px; padding-top: 4px; padding-bottom: 4px; padding-right: 12px; box-sizing: border-box; }
.pdf-tab .pdf-layer-row:hover { background: var(--color-row-hover); }
.pdf-tab .pdf-wrap { overflow-wrap: anywhere; color: var(--color-text-primary); }
.pdf-tab .pdf-layer-row[data-empty] .pdf-wrap { color: var(--color-text-secondary); }
.pdf-tab .pdf-empty-group { margin-top: 6px; border-top: 1px solid var(--color-border); }
.pdf-tab .pdf-group-btn { display: flex; width: 100%; align-items: center; gap: 6px; padding: 8px 14px; text-align: left; color: var(--color-text-secondary); cursor: pointer; border-radius: var(--radius-selector); }
.pdf-tab .pdf-filters { display: flex; flex-wrap: wrap; gap: 6px; padding: 8px 12px 4px 14px; }
.pdf-tab .pdf-group-head { padding: 10px 14px 4px; color: var(--color-text-secondary); font-weight: 600; }
.pdf-tab .pdf-annot-row { display: flex; flex-direction: column; align-items: stretch; gap: 2px; width: 100%; padding: 6px 14px; text-align: left; cursor: pointer; }
.pdf-tab .pdf-row-on { background: color-mix(in srgb, var(--color-accent) 12%, transparent); }
.pdf-tab .pdf-annot-line { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 6px; min-width: 0; color: var(--color-text-primary); }
.pdf-tab .pdf-annot-text { color: var(--color-text-primary); overflow-wrap: anywhere; min-width: 0; }
.pdf-tab .pdf-thumbs { display: grid; grid-template-columns: repeat(auto-fill, minmax(132px, 1fr)); gap: 10px; padding: 10px 14px; }
.pdf-tab .pdf-thumb { display: flex; width: 100%; flex-direction: column; align-items: center; gap: 4px; padding: 6px; border: 1px solid transparent; border-radius: var(--radius-field); background: transparent; color: var(--color-text-secondary); cursor: pointer; }
.pdf-tab .pdf-thumb:hover { background: var(--color-row-hover); }
.pdf-tab .pdf-thumb-on { border-color: var(--color-accent-text); color: var(--color-text-primary); }
.pdf-tab .pdf-thumb-canvas { display: block; min-height: 40px; box-shadow: 0 0 0 1px var(--color-border); }
`

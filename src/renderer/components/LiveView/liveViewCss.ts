// The live workbook view's own styles: the grid's lines, the presence and
// change marks, and their motion. Every colour, radius and duration is a
// token (globals.css / the Style package); a peer's colour comes from the
// data as `--c`. Reduced motion turns every animation off. Rendered once per
// view as a <style> element, scoped under `.lv-root`.

export const LIVE_VIEW_CSS = `
.lv-root { --lv-line: var(--color-divider); --lv-head: var(--color-bg-stripe); --lv-cell: var(--color-bg); }
.lv-root .lv-num { font-variant-numeric: tabular-nums; }
.lv-scroll { position: relative; overflow: auto; overscroll-behavior: contain; background: var(--lv-cell); }
.lv-scroll:focus-visible { outline: none; box-shadow: var(--ring-focus); }
.lv-head-row { position: sticky; top: 0; z-index: 4; display: flex; background: var(--lv-head); border-bottom: 1px solid var(--color-border); }
.lv-corner { position: sticky; left: 0; z-index: 5; flex: none; background: var(--lv-head); border-right: 1px solid var(--color-border); }
.lv-letter, .lv-rownum { position: absolute; display: flex; align-items: center; justify-content: center; color: var(--color-text-muted); font-size: 11px; }
.lv-letter { top: 0; bottom: 0; border-right: 1px solid var(--lv-line); }
.lv-letter[data-cur], .lv-rownum[data-cur] { color: var(--color-accent-text); background: var(--color-accent-soft); font-weight: 600; }
.lv-gutter { position: sticky; left: 0; z-index: 3; flex: none; background: var(--lv-head); border-right: 1px solid var(--color-border); }
.lv-rownum { left: 0; right: 0; border-bottom: 1px solid var(--lv-line); overflow: hidden; }
.lv-rownum[data-hidden] { border-bottom: 2px solid var(--color-border-strong); }
.lv-rownum[data-renum] { animation: lv-renum var(--motion-duration-slow) var(--motion-ease-standard) 2; color: var(--rc, var(--color-accent)); font-weight: 600; }
.lv-row { position: absolute; left: 0; border-bottom: 1px solid var(--lv-line); }
.lv-row[data-slide] { animation: lv-slide var(--motion-duration-slow) var(--motion-ease-out-soft); }
.lv-row[data-enter] { animation: lv-enter var(--motion-duration-slow) var(--motion-ease-out-soft); }
.lv-vline { position: absolute; top: 0; bottom: 0; width: 1px; background: var(--lv-line); }
.lv-cell { position: absolute; top: 0; bottom: 0; padding: 0 6px; display: flex; align-items: center; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; color: var(--color-text-primary); font-size: 12px; }
.lv-cell[data-num] { justify-content: flex-end; font-variant-numeric: tabular-nums; }
.lv-cell[data-cached] { color: var(--color-text-secondary); }
.lv-cell[data-err] { color: var(--color-status-error-text); }
.lv-cell[data-nc] { color: var(--color-status-warn-text); font-style: italic; font-size: 11px; justify-content: flex-end; }
.lv-cell[data-fml] { font-family: var(--font-code); font-size: 11px; color: var(--color-accent-text); }
.lv-cell[data-bold] { font-weight: 600; }
.lv-cell[data-merge] { justify-content: center; z-index: 1; background: var(--lv-fill, var(--lv-cell)); }
.lv-ovl { position: absolute; pointer-events: none; box-sizing: border-box; }
.lv-range { background: color-mix(in srgb, var(--c) 12%, transparent); border: 1px solid color-mix(in srgb, var(--c) 70%, transparent); }
.lv-caret { border: 2px solid var(--c); z-index: 2; }
.lv-caret[data-agent] { border-width: 3px; }
.lv-self { border: 2px solid var(--color-accent); z-index: 2; }
.lv-claim { border: 1.5px dashed var(--c); background: repeating-linear-gradient(135deg, color-mix(in srgb, var(--c) 10%, transparent) 0 6px, transparent 6px 12px); }
.lv-rowband { background: color-mix(in srgb, var(--c) 8%, transparent); }
.lv-chg { border: 2px solid var(--c); background: color-mix(in srgb, var(--c) 10%, transparent); animation: lv-flash 2.6s var(--motion-ease-standard) forwards; pointer-events: auto; }
.lv-chg[data-old] { animation: none; opacity: .35; background: transparent; }
.lv-band { background: color-mix(in srgb, var(--c) 16%, transparent); animation: lv-flash 2.6s var(--motion-ease-standard) forwards; }
.lv-del { height: 2px; background: var(--color-danger); animation: lv-flash 2.6s var(--motion-ease-standard) forwards; }
.lv-tag { position: absolute; z-index: 6; transform: translateY(-100%); display: inline-flex; align-items: center; gap: 4px; padding: 1px 6px; white-space: nowrap; font-size: 11px; line-height: 16px; color: var(--color-on-accent); background: var(--c); border-radius: var(--radius-badge); pointer-events: none; }
.lv-tag[data-plain] { color: var(--c); background: var(--color-bg-elevated); box-shadow: var(--ring-surface); transform: none; }
.lv-vpb { position: absolute; background: var(--c); opacity: .8; border-radius: var(--radius-pill); }
.lv-av { flex: none; display: inline-flex; align-items: center; justify-content: center; border-radius: var(--radius-pill); background: var(--c); color: var(--color-on-accent); font-weight: 600; line-height: 1; }
.lv-av[data-agent] { border-radius: var(--radius-badge); }
.lv-av[data-ring] { box-shadow: 0 0 0 2px var(--color-bg), 0 0 0 3.5px var(--c); }
.lv-av[data-away] { opacity: .45; }
.lv-tab[aria-selected="true"] { background: var(--color-bg); color: var(--color-text-primary); box-shadow: inset 0 2px 0 var(--color-accent); }
.lv-tab[data-flash] { animation: lv-tabflash 1.6s var(--motion-ease-standard); }
.lv-edge { position: absolute; z-index: 7; }
.lv-mini-dot { position: absolute; background: var(--color-text-muted); opacity: .5; }
.lv-mini-vp { position: absolute; border: 1.5px solid var(--c); background: color-mix(in srgb, var(--c) 10%, transparent); }
.lv-mini-you { position: absolute; border: 1.5px solid var(--color-accent); }
.lv-swap { animation: lv-swap var(--motion-duration-medium) var(--motion-ease-out-soft); }
.lv-hist-item[data-cur] { background: var(--color-accent-soft); }
.lv-skel { position: absolute; top: 7px; height: 8px; background: var(--color-bg-hover); border-radius: var(--radius-badge); animation: lv-pulse 1.4s ease-in-out infinite; }
@keyframes lv-flash { 0% { opacity: 1; } 70% { opacity: .9; } 100% { opacity: .3; } }
@keyframes lv-enter { from { transform: scaleY(0); transform-origin: top; opacity: 0; } to { transform: none; opacity: 1; } }
@keyframes lv-slide { from { transform: translateY(var(--lv-dy, 0px)); } to { transform: none; } }
@keyframes lv-renum { 50% { background: color-mix(in srgb, var(--rc, var(--color-accent)) 18%, transparent); } }
@keyframes lv-tabflash { 30% { box-shadow: inset 0 0 0 2px var(--tc, var(--color-accent)); } }
@keyframes lv-swap { from { opacity: 0; transform: translateY(3px); } to { opacity: 1; transform: none; } }
@keyframes lv-pulse { 50% { opacity: .45; } }
.lv-root[data-reduced] *, .lv-root[data-reduced] *::before { animation: none !important; transition: none !important; }
@media (prefers-reduced-motion: reduce) { .lv-root *, .lv-root *::before { animation: none !important; transition: none !important; } }
`

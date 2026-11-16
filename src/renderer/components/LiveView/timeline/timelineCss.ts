// The timeline's lanes and ruler, scoped under `.lv-tl`. Every colour is a
// token or a mix of tokens: clips are coloured by media type only (video the
// first chart hue, audio the third, titles the warn hue, nests neutral),
// mixed into the page so the same rule reads in every look. People's and
// markers' colours come from the data as `--lv-c` / `--lv-mc`. Terminal's
// cut comes from data-k2-chamfer; Glass rounds with the radius tokens.
// Reduced motion turns every animation off.

export const TIMELINE_CSS = `
.lv-tl {
  --lv-tl-lane: var(--color-bg);
  --lv-tl-alt: var(--color-bg-stripe, var(--color-bg));
  --lv-tl-ruler: var(--color-bg-surface);
  --lv-tl-tick: var(--color-border-strong);
  --lv-tl-ink: var(--color-text-secondary);
  --lv-tl-v: color-mix(in srgb, var(--color-chart-1) 30%, var(--color-bg));
  --lv-tl-a: color-mix(in srgb, var(--color-chart-3) 30%, var(--color-bg));
  --lv-tl-g: color-mix(in srgb, var(--color-status-warn) 30%, var(--color-bg));
  --lv-tl-n: color-mix(in srgb, var(--color-text-secondary) 20%, var(--color-bg));
  --lv-tl-edge: color-mix(in srgb, var(--color-text-primary) 18%, transparent);
  --lv-tl-menu: var(--color-bg);
  /* small corners: none in Terminal (its selector radius is 0), about 4 and 5 px in Glass */
  --lv-tl-r: calc(var(--radius-selector) / 4);
  --lv-tl-r2: calc(var(--radius-selector) / 3);
}
.lv-tl .lv-num { font-variant-numeric: tabular-nums; }
.lv-tl-bar { border-top: 1px solid var(--color-border); }
.lv-tl-lanes { position: relative; border-top: 1px solid var(--color-border); background: var(--lv-tl-lane); outline: none; }
.lv-tl-lanes:focus-visible::after { z-index: 30; }

/* the ruler: flags and tags on the top row, timecode labels, the change band, ticks rising from the bottom, the readiness line */
.lv-tl-ruler { position: relative; display: flex; height: 56px; background: var(--lv-tl-ruler); border-bottom: 1px solid var(--lv-tl-tick); }
.lv-tl-corner { flex: none; display: flex; align-items: center; padding: 0 10px 0 12px; border-right: 1px solid var(--lv-tl-tick); background: var(--lv-tl-ruler); min-width: 0; }
.lv-tl-counter { font: 600 20px/1 var(--font-ui); font-variant-numeric: tabular-nums; color: var(--color-text-primary); white-space: nowrap; letter-spacing: .01em; }
.lv-tl-rclip { position: relative; flex: 1; min-width: 0; overflow: hidden; }
.lv-tl-rbody { position: absolute; left: 0; top: 0; bottom: 0; will-change: transform; }
.lv-tl-tick { position: absolute; bottom: 3px; width: 1px; height: 4px; background: var(--lv-tl-tick); }
.lv-tl-tick[data-major] { height: 10px; background: color-mix(in srgb, var(--lv-tl-ink) 60%, transparent); }
.lv-tl-tl { position: absolute; top: 22px; font: 400 11px/1 var(--font-ui); font-variant-numeric: tabular-nums; color: var(--lv-tl-ink); white-space: nowrap; transform: translateX(3px); }
.lv-tl-tag { position: absolute; top: 1px; height: 18px; box-sizing: border-box; padding: 0 6px; font: 500 12px/18px var(--font-ui); white-space: nowrap; overflow: hidden; text-align: center; border-radius: var(--radius-pill); z-index: 3; }
.lv-tl-tag[data-person] { background: var(--lv-c); color: var(--lv-ink); --k2-ch-bw: 0px; }
.lv-tl-tag[data-person][data-away] { filter: grayscale(1); }
.lv-tl-tag[data-marker] { background: var(--color-bg-elevated); color: var(--color-text-primary); border: 1px solid var(--color-border-strong); padding: 0 5px; line-height: 16px; z-index: 2; }
.lv-tl-flag { position: absolute; top: 4px; width: 8px; height: 12px; margin-left: -4px; color: var(--lv-mc); z-index: 2; }
.lv-tl-flag svg { display: block; width: 8px; height: 12px; }
.lv-tl-brk { position: absolute; height: 6px; box-sizing: border-box; border: 2px solid var(--lv-c); border-bottom: 0; background: color-mix(in srgb, var(--lv-c) 10%, transparent); opacity: .75; }
.lv-tl-brk[data-fresh] { opacity: 1; background: color-mix(in srgb, var(--lv-c) 28%, transparent); }
.lv-tl-brk[data-testid] { cursor: pointer; }
.lv-tl-brk:hover { opacity: 1; background: color-mix(in srgb, var(--lv-c) 36%, transparent); }
.lv-tl-loop { position: absolute; top: 18px; bottom: 2px; background: linear-gradient(to right, color-mix(in srgb, var(--color-text-primary) 16%, transparent) var(--lv-lead), color-mix(in srgb, var(--color-text-primary) 8%, transparent) var(--lv-lead), color-mix(in srgb, var(--color-text-primary) 8%, transparent) calc(100% - var(--lv-lead)), color-mix(in srgb, var(--color-text-primary) 16%, transparent) calc(100% - var(--lv-lead))); pointer-events: none; }
.lv-tl-rdy { position: absolute; bottom: 0; height: 2px; }
.lv-tl-rdy[data-kind="ok"] { background: color-mix(in srgb, var(--color-status-ok) 75%, transparent); }
.lv-tl-rdy[data-kind="stills"] { background: color-mix(in srgb, var(--color-status-warn) 80%, transparent); }
.lv-tl-rdy[data-kind="none"] { background: color-mix(in srgb, var(--color-status-error) 75%, transparent); }
.lv-tl-mon { position: absolute; bottom: 0; width: 12px; height: 9px; margin-left: -6px; color: var(--lv-c); z-index: 4; }
.lv-tl-mon svg { display: block; width: 12px; height: 9px; }
.lv-tl .lv-edge { position: absolute; top: 0; height: 20px; z-index: 6; }

/* the lanes */
.lv-tl-canvas { position: relative; }
/* The scroller's size never depends on its content: the horizontal bar is always there and the vertical one keeps its gutter, so the lanes' height and width can't flip-flop as a bar comes and goes. */
.lv-tl-scroller { overflow-x: scroll; overflow-y: auto; scrollbar-gutter: stable; }
.lv-tl-lane { position: relative; display: flex; border-bottom: 1px solid var(--color-border); }
.lv-tl-lane[data-split] { border-top: 3px solid color-mix(in srgb, var(--lv-tl-ink) 30%, var(--lv-tl-lane)); }
.lv-tl-lane:nth-child(odd) .lv-tl-body { background: var(--lv-tl-alt); }
.lv-tl-head { position: sticky; left: 0; z-index: 7; flex: none; display: flex; align-items: center; gap: 10px; padding: 0 12px; box-sizing: border-box; background: var(--lv-tl-ruler); border-right: 1px solid var(--lv-tl-tick); min-width: 0; }
.lv-tl-head[data-narrow] { flex-direction: column; justify-content: center; align-items: center; gap: 3px; padding: 0 4px; }
.lv-tl-chip { flex: none; display: inline-grid; place-items: center; width: 30px; height: 20px; border-radius: var(--lv-tl-r2); font: 700 11px/1 var(--font-ui); color: var(--color-text-primary); background: var(--lv-tl-v); --k2-ch-bw: 0px; }
.lv-tl-lane[data-kind="audio"] .lv-tl-chip { background: var(--lv-tl-a); }
.lv-tl-lane[data-size="S"] .lv-tl-chip { width: 26px; height: 16px; font-size: 10px; }
.lv-tl-hname { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; font-size: 12px; color: var(--color-text-secondary); white-space: nowrap; overflow: hidden; }
.lv-tl-lane[data-size="S"] .lv-tl-hname { font-size: 11px; }
.lv-tl-hflag { flex: none; display: inline-grid; place-items: center; width: 22px; height: 18px; border-radius: var(--radius-pill); border: 1px solid var(--color-control-edge); color: var(--color-text-secondary); --k2-ch-bw: 1px; }
.lv-tl-head[data-narrow] .lv-tl-chip { width: 26px; }
.lv-tl-head[data-narrow] .lv-tl-hflag { width: 18px; }
.lv-tl-body { position: relative; flex: none; }
.lv-tl-clip { position: absolute; top: 2px; bottom: 2px; overflow: hidden; border-radius: var(--lv-tl-r); background: var(--lv-tl-v); color: var(--color-text-primary); box-shadow: inset 0 1px 0 color-mix(in srgb, var(--color-text-primary) 16%, transparent), inset 0 0 0 1px var(--lv-tl-edge); z-index: 1; }
.lv-tl-clip[data-kind="audio"] { background: var(--lv-tl-a); }
.lv-tl-clip[data-kind="title"] { background: var(--lv-tl-g); }
.lv-tl-clip[data-kind="nest"] { background: var(--lv-tl-n); }
.lv-tl-clip[data-off] { background: color-mix(in srgb, var(--lv-tl-v) 45%, var(--lv-tl-lane)); }
.lv-tl-clip[data-off] canvas { opacity: .35; }
.lv-tl-clip[data-off]::after { content: ''; position: absolute; inset: 0; background: repeating-linear-gradient(135deg, transparent 0 5px, color-mix(in srgb, currentColor 18%, transparent) 5px 6px); z-index: 2; pointer-events: none; }
.lv-tl-clip[data-off] .lv-tl-name { text-decoration: line-through; text-decoration-color: color-mix(in srgb, currentColor 60%, transparent); }
.lv-tl-clip[data-held] { outline: 1.5px dashed var(--lv-hc); outline-offset: -2px; z-index: 2; }
.lv-tl-clip[data-changed] { box-shadow: inset 0 0 0 2px var(--lv-cc); z-index: 3; }
.lv-tl-clip[data-refused] { box-shadow: inset 0 0 0 2px var(--color-status-error); z-index: 3; }
.lv-tl-clip[data-released] { outline: 2px dashed var(--lv-rc); outline-offset: -2px; animation: lv-tl-fade 2.4s var(--motion-ease-standard) 1.2s forwards; }
.lv-tl-band { position: absolute; top: 0; height: 16px; display: flex; align-items: center; gap: 4px; padding: 0 4px 0 6px; box-sizing: border-box; z-index: 3; min-width: 0; }
.lv-tl-name { flex: 0 1 auto; min-width: 0; overflow: hidden; white-space: nowrap; font: 500 11px/16px var(--font-ui); }
.lv-tl-kind { flex: none; display: inline-flex; width: 10px; height: 10px; opacity: .85; }
.lv-tl-kind svg { width: 10px; height: 10px; }
.lv-tl-dot { flex: none; width: 8px; height: 8px; border-radius: 50%; background: var(--lv-cc); box-shadow: 0 0 0 1px var(--color-bg); }
.lv-tl-lock { flex: none; display: inline-grid; place-items: center; width: 14px; height: 12px; border-radius: var(--lv-tl-r); background: var(--lv-hc); color: var(--lv-hink); --k2-ch-bw: 0px; }
.lv-tl-lock svg { width: 8px; height: 8px; }
.lv-tl-stop { flex: none; display: inline-grid; place-items: center; width: 14px; height: 12px; color: var(--color-status-error-text); }
.lv-tl-stop svg { width: 11px; height: 11px; }
.lv-tl-pic { position: absolute; left: 0; display: block; pointer-events: none; }
.lv-tl-wave { position: absolute; left: 0; bottom: 0; display: block; color: color-mix(in srgb, var(--color-text-primary) 55%, transparent); pointer-events: none; }
.lv-tl-pend { position: absolute; left: 0; top: 16px; bottom: 0; background: color-mix(in srgb, var(--color-text-secondary) 30%, var(--lv-tl-v)); }
.lv-tl-trx { position: absolute; top: 18px; bottom: 4px; z-index: 4; pointer-events: none; color: color-mix(in srgb, var(--color-text-primary) 45%, transparent); }
.lv-tl-trx svg { display: block; width: 100%; height: 100%; }
.lv-tl-ghost { position: absolute; top: 2px; bottom: 2px; box-sizing: border-box; border: 1.5px dashed var(--lv-c); border-radius: var(--lv-tl-r); background: color-mix(in srgb, var(--lv-c) 9%, transparent); pointer-events: none; z-index: 0; }
.lv-tl-ghost[data-old] { border-color: color-mix(in srgb, var(--lv-c) 45%, transparent); background: transparent; }
.lv-tl-ghost[data-kind="gone"] { border-color: var(--color-status-error); }
.lv-tl-ghost[data-kind="cut"] { border-style: dotted; }
.lv-tl-imk { position: absolute; bottom: 0; width: 8px; height: 6px; margin-left: -4px; color: var(--lv-mc); z-index: 4; }
.lv-tl-imk svg, .lv-tl-tmk svg { display: block; width: 100%; height: 100%; }
.lv-tl-tmk { position: absolute; bottom: 2px; width: 8px; height: 8px; margin-left: -4px; color: var(--lv-mc); z-index: 4; }
.lv-tl-lines { position: absolute; top: 0; pointer-events: none; z-index: 5; }
.lv-tl-ph { position: absolute; top: 0; bottom: 0; width: 0; border-left: 1px solid var(--lv-c); }
.lv-tl-ph[data-away] { opacity: .4; border-left-style: dashed; }
.lv-tl-ph[data-mon] { border-left-width: 2px; margin-left: -.5px; }
.lv-tl-mkline { position: absolute; top: 0; bottom: 0; width: 0; border-left: 1px dashed color-mix(in srgb, var(--lv-mc) 70%, transparent); }
.lv-tl-more { position: absolute; left: 50%; bottom: 8px; transform: translateX(-50%); z-index: 13; display: inline-flex; align-items: center; gap: 6px; height: 26px; padding: 0 10px 0 8px; font-size: 12px; border-radius: var(--radius-pill); border: 1px solid var(--color-control-edge); background: var(--color-bg-elevated); color: var(--color-text-primary); box-shadow: var(--shadow-2); cursor: pointer; }
.lv-tl-more:hover { background: var(--color-control-hover); }
.lv-tl-ovw { position: relative; height: 14px; border-top: 1px solid var(--color-border); background: var(--lv-tl-lane); }
.lv-tl-ovw canvas { position: absolute; inset: 0; width: 100%; height: 100%; cursor: pointer; }
.lv-tl-ov-v { background: var(--lv-tl-v); }
.lv-tl-ov-a { background: var(--lv-tl-a); }
.lv-tl-ov-g { background: var(--lv-tl-g); }
.lv-tl-ovv { position: absolute; top: 0; bottom: 0; box-sizing: border-box; border: 1px solid var(--color-text-secondary); background: color-mix(in srgb, var(--color-text-primary) 7%, transparent); pointer-events: none; }
.lv-tl-ovh { position: absolute; top: 0; bottom: 0; width: 0; border-left: 1px solid var(--lv-c); pointer-events: none; }
.lv-tl-probe { position: absolute; left: 0; top: 0; visibility: hidden; pointer-events: none; white-space: nowrap; }
.lv-tl-tagfont { font: 500 12px/18px var(--font-ui); }

/* the View menu: three radio groups; the chosen fill is the accent that reads on the menu, the mark the menu's own colour; the focus ring 2px, 2px out, in the radio's shape */
.lv-tl-vmenu { position: absolute; right: 0; bottom: calc(100% + 6px); z-index: 45; width: 248px; box-sizing: border-box; font-size: 12px; color: var(--color-text-primary); }
.lv-tl-vmenu[data-down] { bottom: auto; top: calc(100% + 6px); }
.lv-tl-vbody { padding: 8px 6px; }
.lv-tl-vmenu fieldset { border: 0; margin: 0 0 6px; padding: 0; min-width: 0; }
.lv-tl-vmenu legend { font-weight: 650; padding: 0 6px 2px; font-size: 12px; color: var(--color-text-primary); }
.lv-tl-vopt { position: relative; display: flex; align-items: center; gap: 9px; padding: 4px 6px; cursor: pointer; border-radius: var(--radius-field); }
.lv-tl-vopt:hover { background: var(--color-row-hover); }
.lv-tl-vopt .k2-radio { position: relative; z-index: 1; }
.lv-tl-vmenu .k2-radio:checked { background-color: var(--color-accent-text); border-color: var(--color-accent-text); --k2-ch-edge: var(--color-accent-text); background-image: linear-gradient(var(--lv-tl-menu), var(--lv-tl-menu)); background-size: 6px 6px; background-position: center; background-repeat: no-repeat; }
html:not([data-style="terminal"]) .lv-tl-vmenu .k2-radio:checked { background-image: radial-gradient(circle, var(--lv-tl-menu) 0 26%, transparent 31%); background-size: 100% 100%; }
.lv-tl-vmenu .k2-radio:focus-visible { box-shadow: none; outline: none; }
.lv-tl-vmenu .k2-radio:not(:checked):focus-visible { border-color: var(--color-control-edge); --k2-ch-edge: var(--color-control-edge); --k2-ch-bw: 1px; }
.lv-tl-ring { position: absolute; left: 2px; top: 50%; width: 22px; height: 22px; margin-top: -11px; display: none; box-sizing: border-box; border-radius: var(--radius-pill); background: var(--color-accent-text); border-color: var(--color-accent-text); --k2-ch-bw: 0px; }
.lv-tl-ring > span { position: absolute; left: 2px; top: 2px; width: 18px; height: 18px; border-radius: var(--radius-pill); background: var(--lv-tl-menu); border-color: var(--lv-tl-menu); --k2-ch-bw: 0px; }
/* Terminal draws the ring itself, cut to the octagon; Glass keeps K2's own round focus ring on the radio (globals.css). */
html[data-style="terminal"] .lv-tl-vopt:has(.k2-radio:focus-visible) .lv-tl-ring { display: block; }
html[data-style="terminal"] .lv-tl-ring { --k2-ch-cut: 5px; }
html[data-style="terminal"] .lv-tl-ring > span { --k2-ch-cut: 4px; }
.lv-tl-vdone { display: flex; align-items: flex-start; gap: 6px; margin: 2px 6px 0; padding-top: 7px; border-top: 1px solid var(--color-border); color: var(--color-text-primary); }
.lv-tl-vdone svg { flex: none; margin-top: 2px; color: var(--color-status-ok); }
@keyframes lv-tl-fade { to { outline-color: transparent; } }
@media (prefers-reduced-motion: reduce) { .lv-tl *, .lv-tl *::before, .lv-tl *::after { animation: none !important; transition: none !important; } }
`

// The monitor and the transport (T4), scoped under `.lv-tl`. The monitor
// is a picture well: black in every look (the design section: black is the
// one colour that means something, a gap), so what sits on the picture
// takes K2's media-label tokens (the ink and chip made for text on
// pictures) rather than the page's. Its focus ring is that ink on a dark
// halo, so it reads on any frame. The transport is on the page: K2's
// controls, the accent-filled Play, one 11 px tabular style for small
// times. Terminal cuts with data-k2-chamfer; Glass rounds with the tokens.

export const MONITOR_CSS = `
.lv-tl-mon-row { gap: 10px; }
.lv-mon {
  --lv-ink: var(--color-media-label-text);
  --lv-ink-2: color-mix(in srgb, var(--color-media-label-text) 80%, transparent);
  --lv-chip: var(--color-media-label-bg);
  --lv-chip-edge: color-mix(in srgb, var(--color-media-label-text) 24%, transparent);
  --lv-card: color-mix(in srgb, var(--color-media-label-text) 10%, black);
  --lv-btn: color-mix(in srgb, var(--color-media-label-text) 18%, black);
  --lv-btn-h: color-mix(in srgb, var(--color-media-label-text) 28%, black);
  --lv-sample: color-mix(in srgb, var(--color-status-warn) 38%, var(--color-media-label-text));
  position: relative; background: black; overflow: hidden; border-radius: var(--radius-box);
  box-shadow: inset 0 0 0 1px var(--color-border);
}
.lv-mon-frame { position: absolute; overflow: hidden; }
.lv-mon-frame canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.lv-mon-frame[data-blank] canvas { opacity: 0; }
.lv-mon-chip { position: absolute; display: inline-flex; align-items: center; gap: 6px; height: 24px; padding: 0 9px; box-sizing: border-box; border-radius: var(--radius-pill); background: var(--lv-chip); color: var(--lv-ink); font: 500 11.5px/1 var(--font-ui); white-space: nowrap; box-shadow: inset 0 0 0 1px var(--lv-chip-edge); border-color: var(--lv-chip-edge); --k2-ch-bw: 1px; z-index: 3; }
.lv-mon-label { left: 10px; top: 10px; max-width: 56%; overflow: hidden; }
.lv-mon-label .lv-mon-lane { flex: none; padding: 0 5px; border-radius: var(--lv-tl-r); font: 700 10px/16px var(--font-ui); background: color-mix(in srgb, var(--color-media-label-text) 18%, transparent); }
.lv-mon-label .lv-mon-clip { overflow: hidden; text-overflow: ellipsis; }
.lv-mon-sample { right: 44px; top: 10px; height: 22px; font-size: 11px; color: var(--lv-sample); }
.lv-mon-info { right: 10px; top: 10px; width: 26px; padding: 0; justify-content: center; cursor: pointer; border: 0; }
.lv-mon-info:hover, .lv-mon-btn:hover { background: var(--lv-btn-h); }
.lv-mon-spots { position: absolute; right: 10px; top: 42px; display: flex; flex-direction: column; align-items: flex-end; gap: 5px; z-index: 3; }
.lv-mon-spot { position: static; height: 22px; padding: 0 9px 0 3px; box-shadow: inset 0 0 0 1.5px var(--lv-c); border-color: var(--lv-c); --k2-ch-bw: 1.5px; }
.lv-mon-note { left: 10px; bottom: 10px; max-width: 60%; overflow: hidden; padding-right: 4px; }
.lv-mon-note[data-late] { animation: lv-mon-in var(--motion-duration-fast) ease-out 300ms both; }
.lv-mon-note .lv-mon-tx { overflow: hidden; text-overflow: ellipsis; padding-right: 5px; }
.lv-mon-prep { left: 10px; bottom: 10px; height: auto; min-height: 24px; max-width: calc(100% - 20px); padding: 4px 6px 4px 10px; flex-direction: column; align-items: flex-start; gap: 4px; border-radius: 12px; white-space: normal; }
html[data-style="terminal"] .lv-mon-prep { border-radius: 0; }
.lv-mon-prep .lv-mon-l { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
.lv-mon-prep .lv-mon-why { max-width: 380px; font-size: 11px; color: var(--lv-ink-2); }
.lv-mon-prep[data-done] .lv-mon-l:first-child { color: color-mix(in srgb, var(--color-status-ok) 50%, var(--color-media-label-text)); }
.lv-mon-btn { display: inline-flex; align-items: center; height: 20px; padding: 0 8px; border: 0; border-radius: var(--radius-pill); background: var(--lv-btn); color: var(--lv-ink); font: 600 11px/20px var(--font-ui); cursor: pointer; white-space: nowrap; --k2-ch-bw: 0px; }
.lv-mon-btn[aria-pressed="true"] { background: var(--lv-btn-h); }
.lv-mon-card { position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); width: min(380px, 80%); box-sizing: border-box; display: flex; flex-direction: column; align-items: center; gap: 6px; text-align: center; padding: 18px 20px; border-radius: var(--radius-box); background: var(--lv-card); color: var(--lv-ink); box-shadow: inset 0 0 0 1px var(--lv-chip-edge); border-color: var(--lv-chip-edge); --k2-ch-bw: 1px; font-size: 12.5px; z-index: 2; }
.lv-mon-card svg { color: var(--lv-ink-2); }
.lv-mon-card[data-tone="bad"] > svg { color: color-mix(in srgb, var(--color-status-error) 60%, var(--color-media-label-text)); }
.lv-mon-card b { font-size: 14px; font-weight: 600; }
.lv-mon-card .lv-mon-sub { color: var(--lv-ink-2); }
.lv-mon-card[data-tone="warn"] > svg { color: color-mix(in srgb, var(--color-status-warn) 60%, var(--color-media-label-text)); }
.lv-mon-btn[data-primary] { background: var(--color-accent-fill); color: var(--color-on-accent); border-color: var(--color-accent-fill); }
.lv-mon-open { width: min(420px, 86%); max-height: calc(100% - 20px); align-items: stretch; text-align: left; gap: 4px; }
.lv-mon-bar { display: block; height: 4px; margin: 6px 0 4px; border-radius: 2px; overflow: hidden; background: color-mix(in srgb, var(--color-media-label-text) 16%, transparent); }
.lv-mon-bar i { display: block; height: 100%; background: var(--color-accent); }
.lv-mon-rows { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 3px; font-size: 12px; min-height: 0; overflow-y: auto; }
/* the rows scroll inside the cut card (a chamfered box itself never scrolls) */
.lv-mon-rows li { display: flex; align-items: center; gap: 8px; }
.lv-mon-rows li[data-state="wait"] { color: var(--lv-ink-2); }
.lv-mon-st { display: inline-grid; place-items: center; width: 14px; height: 14px; flex: none; color: color-mix(in srgb, var(--color-status-ok) 55%, var(--color-media-label-text)); }
.lv-mon-n { margin-left: auto; color: var(--lv-ink-2); }
.lv-mon-kindw { font-size: 11px; color: var(--lv-ink-2); }
.lv-mon-acts { display: flex; gap: 6px; margin-top: 6px; flex-wrap: wrap; justify-content: center; }
.lv-mon-acts .lv-mon-btn { height: 26px; padding: 0 12px; font-size: 12px; font-weight: 500; border: 1px solid color-mix(in srgb, var(--color-media-label-text) 45%, transparent); --k2-ch-bw: 1px; }
.lv-mon-gap { position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); width: max-content; max-width: 90%; text-align: center; color: color-mix(in srgb, var(--color-media-label-text) 66%, black); font: 500 12px/1.4 var(--font-ui); letter-spacing: .02em; z-index: 2; }
.lv-mon-gap .lv-mon-acts .lv-mon-btn { color: var(--lv-ink); }
.lv-mon-first { position: absolute; left: 50%; top: 50%; transform: translate(-50%, -50%); z-index: 2; animation: lv-mon-in var(--motion-duration-fast) ease-out 300ms both; }
.lv-mon-first svg { width: 22px; height: 22px; }
.lv-mon-line { position: absolute; inset: 0; display: grid; place-items: center; padding: 0 24px; text-align: center; color: color-mix(in srgb, var(--color-media-label-text) 72%, black); font-size: 13px; }
/* On the picture, focus is the picture's ink on a dark halo, so it reads on any frame. */
.lv-mon-frame [data-k2-focus]:focus-visible::after { box-shadow: inset 0 0 0 2px var(--lv-ink), 0 0 0 2px black; border-radius: inherit; }
html[data-style="terminal"] .lv-mon-frame [data-k2-focus]:focus-visible::after { background: var(--lv-ink); box-shadow: none; }
@keyframes lv-mon-in { from { opacity: 0; } to { opacity: 1; } }

/* details: K2's float surface over the monitor (a sheet over the tab when narrow or the monitor is short) */
.lv-mon-det { position: absolute; right: 10px; top: 40px; z-index: 20; width: 300px; display: flex; flex-direction: column; box-sizing: border-box; font-size: 11.5px; line-height: 1.4; color: var(--color-text-primary); }
.lv-mon-det[data-sheet] { left: 8px; right: 8px; top: 8px; width: auto; }
.lv-mon-det h4 { display: flex; align-items: center; margin: 0; padding: 8px 8px 4px 12px; font-size: 12px; font-weight: 650; }
.lv-mon-det h4 button { margin-left: auto; }
.lv-mon-det dl { display: grid; grid-template-columns: 104px 1fr; gap: 3px 10px; margin: 0; padding: 2px 12px 8px; }
.lv-mon-det dt { color: var(--color-text-secondary); }
.lv-mon-det dd { margin: 0; overflow-wrap: anywhere; font-variant-numeric: tabular-nums; }
.lv-mon-more { display: flex; align-items: center; gap: 4px; padding: 2px 12px 6px; color: var(--color-text-secondary); font-size: 11px; }

/* the transport: the scrub bar of the whole edit, then whose playhead, the buttons centred on the picture, and the view's settings */
.lv-tl-transport { flex: none; display: flex; flex-direction: column; padding: 4px 0 0; min-width: 0; }
.lv-tl-scrub { position: relative; height: 14px; cursor: pointer; flex: none; margin: 0 2px; outline: none; }
.lv-tl-scrub i { position: absolute; pointer-events: none; }
.lv-tl-scrub .lv-sl { top: 3px; height: 8px; background: color-mix(in srgb, var(--color-text-primary) 18%, transparent); border-radius: 2px; }
.lv-tl-scrub .lv-sb { left: 0; right: 0; top: 6px; height: 2px; border-radius: 1px; background: var(--color-border-strong); }
.lv-tl-scrub .lv-sc { top: 5px; height: 4px; background: var(--lv-c); opacity: .85; border-radius: 1px; }
.lv-tl-scrub .lv-sm { top: 1px; width: 2px; height: 5px; margin-left: -1px; background: var(--lv-mc); }
.lv-tl-scrub .lv-sp { top: 3px; width: 2px; height: 8px; margin-left: -1px; background: var(--lv-c); border-radius: 1px; }
.lv-tl-scrub .lv-sh { top: 1px; width: 3px; height: 12px; margin-left: -1.5px; background: var(--color-text-primary); border-radius: 2px; box-shadow: 0 0 0 1.5px var(--color-bg); }
html[data-style="terminal"] .lv-tl-scrub i { border-radius: 0; }
.lv-tl-trow { height: 44px; display: grid; grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr); align-items: center; gap: 12px; min-width: 0; }
.lv-tl-tleft { justify-self: start; display: flex; align-items: center; gap: 8px; min-width: 0; max-width: 100%; overflow: hidden; }
.lv-tl-tleft .lv-tl-tpl { display: flex; flex-direction: column; min-width: 0; line-height: 1.15; }
.lv-tl-tpw { font-size: 11px; color: var(--color-text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.lv-tl-tcs { font: 500 11px/1.2 var(--font-ui); font-variant-numeric: tabular-nums; color: var(--color-text-primary); }
.lv-tl-tbtns { display: flex; align-items: center; gap: 4px; }
.lv-tl-tb { width: 30px; height: 30px; padding: 0 !important; display: inline-grid !important; place-items: center; border-radius: var(--radius-pill) !important; }
.lv-tl-tb[data-size="sm"] { width: 26px; height: 26px; }
.lv-tl-tb[data-play] { width: 34px; height: 34px; }
.lv-tl-tb:disabled { cursor: default; }
.lv-tl-tright { justify-self: stretch; display: flex; align-items: center; gap: 8px; min-width: 0; padding-block: 5px; margin-block: -5px; overflow: hidden; }
.lv-tl-tright > * { flex: none; }
/* right-aligned, but any overflow runs to the right, where it can be measured and folded */
.lv-tl-tright > :first-child { margin-left: auto; }
.lv-tl-mode { display: inline-flex; align-items: center; height: 22px; padding: 0 9px; box-sizing: border-box; border-radius: var(--radius-pill); border: 1px solid var(--color-control-edge); --k2-ch-bw: 1px; color: var(--color-text-secondary); font-size: 11px; white-space: nowrap; min-width: 108px; justify-content: center; }
.lv-tl-khint { display: inline-grid; place-items: center; width: 22px; height: 22px; color: var(--color-text-secondary); border-radius: var(--radius-field); }
.lv-tl-tright[data-fold] .lv-tl-khint { display: none; }
.lv-tl-tright:not([data-fold="1"])[data-fold] [data-zoom] { display: none; }
.lv-tl-tright:is([data-fold="3"], [data-fold="4"], [data-fold="5"]) [data-testid="lv-loop"] { display: none !important; }
.lv-tl-tright:is([data-fold="4"], [data-fold="5"]) .lv-tl-mode { display: none; }
.lv-tl-tright[data-fold="5"] > :has(> [data-testid="lv-view-btn"]) { display: none; }
`

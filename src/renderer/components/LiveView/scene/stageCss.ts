// The scene's stage and everything on it, scoped under `.lv-scene-hero`:
// the dark stage (it carries the look's dark palette as data attributes, so
// every token below resolves dark in the light looks too), the letterboxed
// still with its 16 px faded margin, the view picker and its menu, the
// compass, the state chips, the key hint, ⓘ and its details, the card and
// the quiet line. Glass rounds and frosts; Terminal cuts (the chamfer is
// data-k2-chamfer) and stays flat. Also the panel's Glass tile (316 px,
// rounded) beside the stage. Every colour, radius and duration is a token.

export const STAGE_CSS = `
.lv-scene-hero { display: flex; min-height: 0; flex: 1; flex-direction: column; }
[data-style="glass"] .lv-scene-hero { padding: 10px; }
.lv-scene-hero .lv-sstage { position: relative; flex: 1; min-height: 0; overflow: hidden; color: var(--color-text-primary); background: var(--lv-stage); outline: none; }
.lv-scene-hero .lv-sstage[data-style="glass"] { --lv-stage: color-mix(in srgb, var(--color-bg-surface) 50%, var(--color-bg-elevated)); --lv-plate: var(--color-bg-elevated); --lv-plate-edge: var(--color-ring-edge); --lv-pop: color-mix(in srgb, var(--color-bg-surface) 82%, transparent); border-radius: var(--radius-box); box-shadow: inset 0 0 0 1px var(--tile-edge); }
.lv-scene-hero .lv-sstage[data-style="terminal"] { --lv-stage: var(--color-bg-inset); --lv-plate: var(--color-bg-surface); --lv-plate-edge: var(--color-border); --lv-pop: var(--color-bg); }
.lv-scene-hero .lv-sstage:focus-visible::after { content: ''; position: absolute; inset: 0; z-index: 50; pointer-events: none; border-radius: var(--radius-box); box-shadow: inset 0 0 0 2px var(--color-accent-text); }

/* The still: letterboxed in the pane, its edges fading into the stage */
.lv-scene-hero .lv-frame { position: absolute; }
.lv-scene-hero .lv-frame canvas, .lv-scene-hero .lv-frame .lv-ground { position: absolute; inset: 0; width: 100%; height: 100%; display: block; -webkit-mask-image: linear-gradient(to right, transparent, black 18px, black calc(100% - 18px), transparent), linear-gradient(to bottom, transparent, black 18px, black calc(100% - 18px), transparent); -webkit-mask-composite: source-in; mask-image: linear-gradient(to right, transparent, black 18px, black calc(100% - 18px), transparent), linear-gradient(to bottom, transparent, black 18px, black calc(100% - 18px), transparent); mask-composite: intersect; }
.lv-scene-hero .lv-frame canvas[hidden] { display: none; }
.lv-scene-hero .lv-frame .lv-old { opacity: 0; transition: opacity var(--motion-duration-slow, .42s) var(--motion-ease-standard); }
.lv-scene-hero .lv-frame .lv-old[data-fading] { transition: none; opacity: 1; }
.lv-scene-hero .lv-frame[data-dim] canvas { opacity: .5; filter: saturate(.3); }
.lv-scene-hero .lv-ground line { stroke: color-mix(in srgb, var(--color-text-primary) 7%, transparent); stroke-width: 2; }
.lv-scene-hero .lv-ground circle { fill: color-mix(in srgb, var(--color-text-primary) 35%, transparent); }

/* Plates on the stage: the picker, chips, the key hint, ⓘ, the compass */
.lv-scene-hero .lv-plate { background: var(--lv-plate); color: var(--color-text-primary); border: 1px solid var(--lv-plate-edge); }
[data-style="glass"] .lv-scene-hero .lv-plate { border-radius: var(--radius-pill); box-shadow: var(--shadow-2); }
.lv-scene-hero .lv-vtop { position: absolute; left: 18px; top: 18px; right: 18px; z-index: 20; display: flex; align-items: flex-start; gap: 8px; pointer-events: none; }
.lv-scene-hero .lv-vtop > * { pointer-events: auto; }
.lv-scene-hero .lv-vbtn { display: inline-flex; align-items: center; gap: 7px; height: 30px; padding: 0 9px 0 10px; font: inherit; font-size: var(--type-md); font-weight: 600; cursor: pointer; border-color: var(--color-control-edge); }
.lv-scene-hero .lv-vbtn svg { color: var(--color-text-secondary); }
.lv-scene-hero .lv-vbtn:hover, .lv-scene-hero .lv-vbtn[aria-expanded="true"] { background: var(--color-control-hover); }
.lv-scene-hero .lv-vold { position: absolute; top: 18px; right: 18px; z-index: 13; pointer-events: none; }
.lv-scene-hero .lv-oldpic { display: inline-flex; align-items: center; gap: 6px; height: 26px; padding: 0 10px; font-size: var(--type-sm); font-weight: 600; border-color: var(--color-status-warn); }
.lv-scene-hero .lv-oldpic svg, .lv-scene-hero .lv-chip[data-tone="bad"] svg { color: var(--color-status-warn-text); }
.lv-scene-hero .lv-cmpslot { position: absolute; left: 18px; bottom: 18px; z-index: 12; }
.lv-scene-hero .lv-compass { display: block; width: 44px; height: 44px; }
[data-style="glass"] .lv-scene-hero .lv-compass { border-radius: 50%; }
.lv-scene-hero .lv-compass svg { width: 44px; height: 44px; display: block; overflow: visible; }
.lv-scene-hero .lv-compass line { stroke-width: 2; stroke-linecap: round; stroke: var(--lv-ax); }
.lv-scene-hero .lv-compass circle.lv-ax { fill: var(--lv-ax); }
.lv-scene-hero .lv-compass text { font: 700 7px/1 var(--font-ui); fill: var(--color-text-primary); text-anchor: middle; }
.lv-scene-hero .lv-compass .lv-far { opacity: .45; }
.lv-scene-hero .lv-compass .lv-o { fill: var(--color-text-secondary); }
.lv-scene-hero .lv-compass [data-axis="x"] { --lv-ax: var(--color-status-error); }
.lv-scene-hero .lv-compass [data-axis="y"] { --lv-ax: var(--color-status-ok); }
.lv-scene-hero .lv-compass [data-axis="z"] { --lv-ax: var(--color-chart-1); }
.lv-scene-hero .lv-vfoot { position: absolute; left: 74px; right: 18px; bottom: 18px; z-index: 11; display: flex; align-items: flex-end; gap: 24px; pointer-events: none; }
.lv-scene-hero .lv-vfoot > * { pointer-events: auto; }
.lv-scene-hero .lv-chips { display: flex; flex-wrap: wrap; gap: 6px; min-width: 0; }
.lv-scene-hero .lv-chip { display: inline-flex; align-items: center; gap: 7px; height: 26px; padding: 0 10px; font-size: var(--type-md); white-space: nowrap; }
.lv-scene-hero .lv-chip[data-tone="bad"] { border-color: var(--color-status-warn); }
.lv-scene-hero .lv-chip[data-tone="ok"] svg { color: var(--color-status-ok); }
.lv-scene-hero .lv-chip[data-tone="info"] svg { color: var(--color-text-secondary); }
.lv-scene-hero .lv-lnk { border: 0; background: none; padding: 0; font: inherit; color: var(--color-accent-text); text-decoration: underline; text-underline-offset: 2px; cursor: pointer; }
.lv-scene-hero .lv-lnk:hover { color: var(--color-text-primary); text-decoration-thickness: 2px; }
.lv-scene-hero .lv-spin { width: 11px; height: 11px; flex: none; border-radius: 50%; border: 1.5px solid color-mix(in srgb, var(--color-text-primary) 25%, transparent); border-top-color: var(--color-text-primary); animation: lv-spin .9s linear infinite; }
.lv-scene-hero .lv-sp { flex: 1; }
.lv-scene-hero .lv-keys { display: inline-flex; align-items: center; gap: 4px; height: 26px; padding: 0 10px; font-size: var(--type-sm); color: var(--color-text-secondary); white-space: nowrap; }
.lv-scene-hero kbd { min-width: 16px; padding: 0 4px; margin-left: 6px; text-align: center; font: 600 var(--type-xs)/16px var(--font-code); color: var(--color-text-primary); background: var(--color-bg-surface); border: 1px solid var(--color-control-edge); }
.lv-scene-hero kbd:first-child { margin-left: 0; }
[data-style="glass"] .lv-scene-hero kbd { border-radius: min(var(--radius-field), 5px); font-family: var(--font-ui); }
.lv-scene-hero .lv-infob { display: grid; place-items: center; width: 28px; height: 28px; padding: 0; color: var(--color-text-secondary); cursor: pointer; flex: none; border-color: var(--color-control-edge); }
[data-style="glass"] .lv-scene-hero .lv-infob { border-radius: 50%; }
.lv-scene-hero .lv-infob:hover, .lv-scene-hero .lv-infob[aria-expanded="true"] { background: var(--color-control-hover); color: var(--color-text-primary); }

/* Floating on the stage: the views menu and the details. Glass lets the stage show through (82%, frost); Terminal is its menu */
.lv-scene-hero .lv-pop { position: absolute; z-index: 40; color: var(--color-text-primary); background: var(--lv-pop); border: 1px solid var(--color-border); }
[data-style="glass"] .lv-scene-hero .lv-pop { border: 0.5px solid var(--color-frost-edge); border-radius: var(--radius-box); box-shadow: inset 0 1px 0 var(--color-frost-sheen), var(--shadow-3); -webkit-backdrop-filter: blur(var(--material-blur)) saturate(var(--material-saturate)); backdrop-filter: blur(var(--material-blur)) saturate(var(--material-saturate)); }
.lv-scene-hero .lv-vmenu { left: 18px; top: 56px; width: 340px; max-width: calc(100% - 36px); padding: 6px; }
.lv-scene-hero .lv-vrow { position: relative; display: flex; width: 100%; align-items: center; gap: 10px; padding: 6px; border: 0; background: none; color: var(--color-text-primary); text-align: left; font: inherit; cursor: pointer; border-radius: min(var(--radius-selector), 10px); }
.lv-scene-hero .lv-vrow:disabled { cursor: default; opacity: .55; }
.lv-scene-hero .lv-vrow:not(:disabled):hover, .lv-scene-hero .lv-vrow:focus-visible, .lv-scene-hero .lv-vrow[data-kbd] { background: var(--color-row-hover); outline: none; }
.lv-scene-hero .lv-vrow:focus-visible::after, .lv-scene-hero .lv-vrow[data-kbd]::after { content: ''; position: absolute; inset: 0; pointer-events: none; border-radius: inherit; box-shadow: inset 0 0 0 2px var(--color-accent-text); }
.lv-scene-hero .lv-vrow[aria-checked="true"] { box-shadow: inset 3px 0 0 var(--color-accent); }
.lv-scene-hero .lv-vrow .lv-th { width: 64px; height: 36px; flex: none; overflow: hidden; display: grid; place-items: center; background: var(--lv-stage); color: var(--color-text-muted); box-shadow: inset 0 0 0 1px var(--color-border); border-radius: min(var(--radius-field), 5px); }
.lv-scene-hero .lv-vrow .lv-th canvas { width: 100%; height: 100%; display: block; }
.lv-scene-hero .lv-vt { display: flex; flex: 1; min-width: 0; flex-direction: column; line-height: 1.3; }
.lv-scene-hero .lv-vt b { font-size: var(--type-lg); font-weight: 600; }
.lv-scene-hero .lv-vt span { font-size: var(--type-md); color: var(--color-text-secondary); }
.lv-scene-hero .lv-check { color: var(--color-accent-text); }
.lv-scene-hero .lv-vsep { height: 1px; margin: 4px 6px; background: var(--color-border); }
.lv-scene-hero .lv-vmfoot { margin-top: 4px; padding: 6px 8px 2px; border-top: 1px solid var(--color-border); font-size: var(--type-sm); color: var(--color-text-secondary); text-wrap: pretty; }
.lv-scene-hero .lv-info { right: 18px; bottom: 56px; width: 360px; max-width: calc(100% - 36px); max-height: calc(100% - 112px); display: flex; flex-direction: column; padding: 10px 12px; font-size: var(--type-md); }
.lv-scene-hero .lv-info[data-sheet] { left: 18px; width: auto; bottom: 72px; }
.lv-scene-hero .lv-info h3, .lv-scene-hero .lv-info .lv-forag { flex: none; }
.lv-scene-hero .lv-info h3 { margin: 0 0 6px; font-size: var(--type-lg); font-weight: 650; }
.lv-scene-hero .lv-info-rows { flex: 0 1 auto; min-height: 0; overflow-y: auto; scrollbar-width: thin; }
.lv-scene-hero .lv-info-rows[data-more] { -webkit-mask-image: linear-gradient(to bottom, black calc(100% - 24px), transparent); mask-image: linear-gradient(to bottom, black calc(100% - 24px), transparent); }
.lv-scene-hero .lv-info dl { margin: 0; display: grid; grid-template-columns: 92px 1fr; gap: 3px 10px; }
.lv-scene-hero .lv-info dt { color: var(--color-text-secondary); }
.lv-scene-hero .lv-info dd { margin: 0; color: var(--color-text-primary); text-wrap: balance; overflow-wrap: anywhere; }
.lv-scene-hero .lv-info .lv-forag { margin-top: 8px; padding-top: 8px; border-top: 1px solid var(--color-border); display: flex; align-items: center; gap: 10px; font-size: var(--type-sm); color: var(--color-text-secondary); }

/* The card over the stage, and the quiet line */
.lv-scene-hero .lv-card { position: absolute; left: 50%; top: 50%; z-index: 9; transform: translate(-50%, -50%); width: min(400px, calc(100% - 36px)); padding: 16px 18px; background: var(--lv-plate); border: 1px solid var(--lv-plate-edge); color: var(--color-text-primary); }
[data-style="glass"] .lv-scene-hero .lv-card { border-radius: var(--radius-box); box-shadow: var(--shadow-3); }
.lv-scene-hero .lv-card[data-top] { top: 104px; transform: translateX(-50%); }
.lv-scene-hero .lv-card[data-quiet] { width: auto; padding: 12px 16px; }
.lv-scene-hero .lv-card h3 { margin: 0 0 4px; font-size: var(--type-xl); font-weight: 650; }
.lv-scene-hero .lv-card p { margin: 0 0 4px; font-size: var(--type-lg); color: var(--color-text-secondary); text-wrap: pretty; }
.lv-scene-hero .lv-card .lv-big { display: grid; place-items: center; width: 36px; height: 36px; margin-bottom: 10px; color: var(--color-text-secondary); background: var(--color-bg-surface); border-radius: min(var(--radius-field), 10px); }
.lv-scene-hero .lv-card .lv-big[data-bad] { color: var(--color-status-warn-text); }
.lv-scene-hero .lv-card .lv-btns { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 12px; }
.lv-scene-hero .lv-steps { list-style: none; margin: 8px 0 0; padding: 0; display: grid; gap: 5px; font-size: var(--type-md); }
.lv-scene-hero .lv-steps li { display: flex; align-items: center; gap: 8px; color: var(--color-text-secondary); }
.lv-scene-hero .lv-steps li[data-state="now"] { color: var(--color-text-primary); font-weight: 600; }
.lv-scene-hero .lv-steps li[data-state="done"] svg { color: var(--color-status-ok); }
.lv-scene-hero .lv-steps .lv-pend { width: 11px; height: 11px; flex: none; border-radius: 50%; border: 1.5px solid var(--color-text-muted); }
.lv-scene-hero .lv-skel { position: absolute; inset: 0; background: linear-gradient(100deg, transparent 30%, color-mix(in srgb, var(--color-text-primary) 5%, transparent) 50%, transparent 70%); background-size: 220% 100%; animation: lv-shimmer 1.8s linear infinite; }
.lv-scene-hero .lv-tslot { position: absolute; left: 50%; bottom: 74px; z-index: 22; transform: translateX(-50%); pointer-events: none; }
.lv-scene-hero .lv-toast { display: inline-flex; align-items: center; gap: 8px; min-height: 32px; padding: 6px 12px 6px 10px; font-size: var(--type-lg); white-space: nowrap; }
.lv-scene-hero .lv-toast svg { color: var(--color-status-ok); }

/* The panel beside the stage: in Glass a 316 px tile with the box radius, in Terminal square with its edge */
[data-style="glass"] .lv-shell .lv-panel:has(.lv-scene-panel):not([data-sheet]) { width: 316px; margin: 10px 10px 10px 0; border-left: 0; background: var(--tile-fill); border-radius: var(--radius-box); box-shadow: inset 0 0 0 1px var(--tile-edge); overflow: hidden; }
[data-style="glass"] .lv-shell .lv-panel:has(.lv-scene-panel)[data-sheet] { border-radius: var(--radius-box); overflow: hidden; }
@keyframes lv-spin { to { transform: rotate(360deg); } }
@keyframes lv-shimmer { from { background-position: 100% 0; } to { background-position: -120% 0; } }
`

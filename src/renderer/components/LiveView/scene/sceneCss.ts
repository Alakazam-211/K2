// The scene panel's styles, scoped under `.lv-scene-panel` (inside the
// shell's `.lv-shell`): section heads, the outliner's rows, Properties'
// transform table, People and Activity, as the mock draws them (v2.5).
// Every colour, radius and size is a token; a person's colour comes from
// the data as `--lv-c`. Your own selection (`--lv-mine`) is a presence colour,
// the warm one (`--color-presence-4`, the mock's orange), not a status
// colour: warn means something is wrong. Object kinds take the chart
// palette (a mesh blue, a light amber, a camera violet mixed from it).

export const SCENE_PANEL_CSS = `
.lv-scene-panel { --lv-mine: var(--color-presence-4); --lv-row: 26px; display: flex; flex-direction: column; min-height: 100%; padding-bottom: 8px; font-size: var(--type-md); }
.lv-scene-panel .lv-ssec { border-bottom: 1px solid var(--color-border); flex: none; }
.lv-scene-panel .lv-ssec:last-of-type { border-bottom: 0; }
.lv-scene-panel .lv-foot { flex: 1 1 auto; }
.lv-scene-panel .lv-shead { position: relative; display: flex; align-items: center; gap: 6px; width: 100%; height: 30px; padding: 0 12px 0 10px; border: 0; background: none; text-align: left; font: inherit; font-size: var(--type-xs); font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--color-text-secondary); cursor: pointer; }
.lv-scene-panel .lv-shead:hover, .lv-scene-panel .lv-shead:focus-visible { background: var(--color-row-hover); color: var(--color-text-primary); }
.lv-scene-panel .lv-shead .lv-chev { display: inline-flex; transition: transform var(--motion-duration-fast, .15s) var(--motion-ease-standard); }
.lv-scene-panel .lv-shead[aria-expanded="false"] .lv-chev { transform: rotate(-90deg); }
.lv-scene-panel .lv-scount { margin-left: auto; font-size: var(--type-xs); font-weight: 500; letter-spacing: 0; text-transform: none; font-variant-numeric: tabular-nums; }
.lv-scene-panel .lv-sbody { padding: 0 12px 10px; color: var(--color-text-secondary); }
.lv-scene-panel .lv-sbody[data-flush] { padding: 0 0 4px; }
.lv-scene-panel .lv-nopick { color: var(--color-text-secondary); text-wrap: pretty; }
.lv-scene-panel .lv-skel { display: grid; gap: 7px; padding: 4px 0 2px; }
.lv-scene-panel .lv-skel i { display: block; height: 8px; width: 80%; background: var(--color-bg-hover); border-radius: min(var(--radius-field), 4px); }
.lv-scene-panel .lv-skel i:nth-child(2) { width: 62%; }
.lv-scene-panel .lv-skel i:nth-child(3) { width: 70%; }

/* Activity */
.lv-scene-panel .lv-arow { display: flex; align-items: flex-start; gap: 7px; padding: 4px 6px; margin: 0 -6px; font-size: var(--type-sm); color: var(--color-text-secondary); }
.lv-scene-panel .lv-arow[data-kind="now"] { box-shadow: inset 2px 0 0 var(--color-accent); }
.lv-scene-panel .lv-arow[data-cur] { box-shadow: inset 2px 0 0 var(--color-accent); }
.lv-scene-panel .lv-arow b { color: var(--color-text-primary); font-weight: 600; }
.lv-scene-panel .lv-at { flex: 1; min-width: 0; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; text-wrap: pretty; }
.lv-scene-panel .lv-tm { flex: none; margin-top: 1px; font-size: var(--type-xs); font-variant-numeric: tabular-nums; color: var(--color-text-secondary); white-space: nowrap; }
.lv-scene-panel .lv-arow[data-kind="refused"] { cursor: help; }
.lv-scene-panel .lv-arow[data-kind="refused"] .lv-tm { color: var(--color-status-warn-text); }
.lv-scene-panel .lv-arow[data-kind="done"] .lv-at { color: var(--color-status-ok); }
.lv-scene-panel .lv-lk { display: inline-grid; place-items: center; width: 16px; height: 16px; flex: none; color: var(--lv-c, var(--color-status-warn-text)); }
.lv-scene-panel .lv-refchip { display: inline-flex; align-items: center; gap: 4px; height: 20px; padding: 0 8px 0 6px; flex: none; font-size: var(--type-xs); font-weight: 600; color: var(--color-text-primary); background: var(--color-row-hover); border: 1px solid var(--color-border); border-radius: var(--radius-pill); white-space: nowrap; }

/* People and holds */
.lv-scene-panel .lv-prow { position: relative; display: flex; align-items: center; gap: 8px; width: calc(100% + 12px); margin: 0 -6px; padding: 4px 6px; border: 0; background: none; text-align: left; font: inherit; color: inherit; border-radius: min(var(--radius-selector), 10px); }
.lv-scene-panel button.lv-prow { cursor: pointer; }
.lv-scene-panel button.lv-prow:hover, .lv-scene-panel button.lv-prow:focus-visible { background: var(--color-row-hover); }
.lv-scene-panel button.lv-prow[aria-pressed="true"] { background: color-mix(in srgb, var(--color-accent) 12%, transparent); }
.lv-scene-panel .lv-prow[data-away] .lv-avatar { opacity: .5; }
.lv-scene-panel .lv-pt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; line-height: 1.25; }
.lv-scene-panel .lv-pl1 { display: flex; align-items: center; gap: 8px; min-width: 0; }
.lv-scene-panel .lv-pl1 b { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 600; color: var(--color-text-primary); }
.lv-scene-panel .lv-pv { display: inline-flex; align-items: center; gap: 4px; flex: none; max-width: 130px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--type-xs); color: var(--color-text-secondary); }
.lv-scene-panel .lv-pwhat { font-size: var(--type-sm); color: var(--color-text-secondary); text-wrap: pretty; }
.lv-scene-panel .lv-pwhat .lv-lk { display: inline-grid; width: 12px; height: 12px; vertical-align: -2px; margin-right: 3px; }
.lv-scene-panel .lv-foltag { flex: none; display: inline-flex; align-items: center; height: 20px; padding: 0 8px; font-size: var(--type-xs); font-weight: 650; color: var(--color-accent-text); border: 1px solid var(--color-control-edge); border-radius: var(--radius-pill); white-space: nowrap; }

/* The outliner */
.lv-scene-panel .lv-otool { padding: 0 10px 6px; }
.lv-scene-panel .lv-olist-box { overflow-y: auto; scrollbar-width: thin; }
.lv-scene-panel .lv-olist { position: relative; outline: none; }
.lv-scene-panel .lv-orow { position: absolute; left: 0; right: 0; height: var(--lv-row); display: grid; grid-template-columns: minmax(0, 1fr) auto 26px; align-items: center; gap: 4px; padding: 0 6px 0 8px; color: var(--color-text-primary); }
.lv-scene-panel .lv-orow[data-obj] { cursor: pointer; }
.lv-scene-panel .lv-orow[data-coll] { font-weight: 600; }
.lv-scene-panel .lv-orow:hover, .lv-scene-panel .lv-orow[data-cur] { background: var(--color-row-hover); }
.lv-scene-panel .lv-orow[aria-selected="true"] { background: color-mix(in srgb, var(--lv-mine) 16%, transparent); box-shadow: inset 2px 0 0 var(--lv-mine); }
.lv-scene-panel .lv-orow[aria-selected="true"] .lv-orow-name { font-weight: 600; }
.lv-scene-panel .lv-olist:focus-visible .lv-orow[data-cur]::after { content: ''; position: absolute; inset: 0; pointer-events: none; box-shadow: inset 0 0 0 2px var(--color-accent-text); border-radius: min(var(--radius-selector), 8px); }
.lv-scene-panel .lv-onm { display: flex; align-items: center; gap: 6px; min-width: 0; height: 100%; }
.lv-scene-panel .lv-orow-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.lv-scene-panel .lv-orow[data-hidden] .lv-orow-name { color: var(--color-text-secondary); }
.lv-scene-panel .lv-orow[data-more] .lv-orow-name { color: var(--color-text-secondary); font-style: italic; }
.lv-scene-panel .lv-ocount { flex: none; font-size: var(--type-xs); font-weight: 500; color: var(--color-text-secondary); font-variant-numeric: tabular-nums; }
.lv-scene-panel .lv-tw { display: grid; place-items: center; width: 14px; height: 14px; flex: none; padding: 0; border: 0; background: none; color: var(--color-text-secondary); cursor: pointer; }
.lv-scene-panel .lv-tw svg { transition: transform var(--motion-duration-fast, .15s) var(--motion-ease-standard); }
.lv-scene-panel .lv-tw[aria-expanded="false"] svg { transform: rotate(-90deg); }
.lv-scene-panel .lv-ti { display: grid; place-items: center; width: 15px; height: 15px; flex: none; color: var(--color-text-secondary); }
.lv-scene-panel .lv-k-mesh { color: var(--color-chart-1); }
.lv-scene-panel .lv-k-camera { color: color-mix(in oklab, var(--color-chart-1) 50%, var(--color-status-error)); }
.lv-scene-panel .lv-k-light { color: var(--color-status-warn-amber); }
.lv-scene-panel .lv-k-empty { color: var(--color-text-secondary); }
.lv-scene-panel .lv-whos { display: flex; align-items: center; gap: 4px; }
.lv-scene-panel .lv-whos .lv-avatar + .lv-avatar { margin-left: -3px; }
.lv-scene-panel .lv-badge { display: inline-grid; place-items: center; width: 16px; height: 16px; flex: none; color: var(--color-text-secondary); background: var(--color-row-hover); border-radius: min(var(--radius-field), 4px); }
.lv-scene-panel .lv-badge[data-hold] { color: var(--lv-c); background: color-mix(in srgb, var(--lv-c) 22%, transparent); }
.lv-scene-panel .lv-adot { display: inline-block; width: 8px; height: 8px; flex: none; border-radius: 50%; background: var(--lv-c); }
.lv-scene-panel .lv-adot[data-acting] { animation: lv-pulse 1.6s var(--motion-ease-standard) infinite; }
.lv-scene-panel .lv-eye { display: grid; place-items: center; width: 22px; height: 20px; color: var(--color-text-secondary); }
.lv-scene-panel .lv-orow[data-flash]::before { content: ''; position: absolute; inset: 0; pointer-events: none; background: color-mix(in srgb, var(--lv-c) 26%, transparent); animation: lv-rflash 2.4s var(--motion-ease-standard) forwards; }
.lv-scene-panel .lv-nohit { padding: 10px 12px 12px; display: flex; flex-direction: column; align-items: flex-start; gap: 6px; color: var(--color-text-secondary); text-wrap: pretty; }
.lv-scene-panel .lv-nohit b { color: var(--color-text-primary); font-weight: 600; }

/* Properties */
.lv-scene-panel .lv-pr1 { display: flex; align-items: center; gap: 7px; margin: 2px 0 8px; }
.lv-scene-panel .lv-pr1[data-mine] .lv-ti { color: var(--lv-mine); }
.lv-scene-panel .lv-pn { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--type-lg); font-weight: 650; color: var(--color-text-primary); }
.lv-scene-panel .lv-psub { margin: 2px 0 4px; padding-bottom: 4px; border-bottom: 1px solid var(--color-border); font-size: var(--type-xs); font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--color-text-secondary); }
.lv-scene-panel .lv-ptab { display: grid; grid-template-columns: minmax(84px, 1fr) repeat(3, minmax(50px, .9fr)); column-gap: 4px; font-size: var(--type-sm); font-variant-numeric: tabular-nums; }
.lv-scene-panel .lv-ptab [role="row"] { display: contents; }
.lv-scene-panel .lv-ptab [role="row"] > * { min-height: 22px; line-height: 22px; }
.lv-scene-panel .lv-ptab [role="rowheader"] { white-space: nowrap; padding-right: 6px; color: var(--color-text-secondary); font-weight: 400; text-align: left; }
.lv-scene-panel .lv-ptab [role="columnheader"] { text-align: right; font-size: var(--type-xs); font-weight: 600; letter-spacing: .05em; color: var(--color-text-secondary); }
.lv-scene-panel .lv-ptab [role="cell"] { text-align: right; color: var(--color-text-primary); }
.lv-scene-panel .lv-ptab [role="cell"][data-changed] { font-weight: 650; background: color-mix(in srgb, var(--lv-c) 18%, transparent); box-shadow: inset 0 -2px 0 var(--lv-c); border-radius: min(var(--radius-field), 2px); }
.lv-scene-panel .lv-ptab [data-before], .lv-scene-panel .lv-ptab [data-before-label] { color: var(--color-text-secondary); text-align: right; }
.lv-scene-panel .lv-ptab [data-before-label] { text-align: left; font-size: var(--type-xs); }
.lv-scene-panel .lv-ptab [data-quat] { grid-column: span 3; text-align: left; font-size: var(--type-sm); color: var(--color-text-primary); }
.lv-scene-panel .lv-pjust { display: flex; align-items: center; gap: 6px; margin-top: 8px; font-size: var(--type-sm); color: var(--color-text-secondary); text-wrap: pretty; }
.lv-scene-panel .lv-pjust b { color: var(--color-text-primary); }
.lv-scene-panel .lv-plim { margin-top: 8px; font-size: var(--type-sm); color: var(--color-text-secondary); text-wrap: pretty; }
@keyframes lv-rflash { from { opacity: 1; } to { opacity: 0; } }
`

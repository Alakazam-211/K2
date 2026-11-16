// S5's styles on the stage, scoped under `.lv-scene-hero` (the dark stage's
// palette, so they read in the light looks too): the panes of one or two
// views (a hairline between two), the chips for who looks through another
// view, the second pane's "what Atlas looks at" label, the finished line,
// the two-views switch, and the turntable strip at the picture's foot.
// Every colour, radius and duration is a token.

export const FOLLOW_CSS = `
.lv-scene-hero .lv-pane { position: absolute; inset: 0; }
.lv-scene-hero .lv-pane[data-split] { inset: auto; overflow: hidden; }
/* A still that covers its half is cropped at the half's edges: no feathered edge, no stage beside it */
.lv-scene-hero .lv-frame[data-cover] canvas, .lv-scene-hero .lv-frame[data-cover] .lv-ground, .lv-scene-hero .lv-pane[data-split] .lv-ovl { -webkit-mask-image: none; mask-image: none; }
.lv-scene-hero .lv-sstage[data-layout="side"] .lv-pane[data-pane="1"] { box-shadow: inset 1px 0 0 var(--color-border); }
.lv-scene-hero .lv-sstage[data-layout="stacked"] .lv-pane[data-pane="1"] { box-shadow: inset 0 1px 0 var(--color-border); }

/* Who looks through another view: each chip follows them */
.lv-scene-hero .lv-vpeer { display: inline-flex; align-items: center; gap: 6px; height: 30px; padding: 0 10px 0 6px; font: inherit; font-size: var(--type-md); font-weight: 500; color: var(--color-text-primary); cursor: pointer; white-space: nowrap; border-color: var(--color-control-edge); }
.lv-scene-hero span.lv-vpeer { cursor: default; padding: 0 10px; color: var(--color-text-secondary); }
.lv-scene-hero button.lv-vpeer:hover { background: var(--color-control-hover); }
.lv-scene-hero .lv-vpeer[data-away] { color: var(--color-text-secondary); }
.lv-scene-hero .lv-plabel { display: inline-flex; align-items: center; height: 30px; padding: 0 10px; font-size: var(--type-md); color: var(--color-text-secondary); white-space: nowrap; }

/* Following ended: the line and its two ways on */
/* Centred across the stage's width (a box at left: 50% would wrap at half the width) */
.lv-scene-hero .lv-fslot { position: absolute; left: 18px; right: 18px; bottom: 74px; z-index: 22; display: flex; justify-content: center; pointer-events: none; }
.lv-scene-hero .lv-fin { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; max-width: 100%; padding: 8px 10px 8px 14px; font-size: var(--type-lg); pointer-events: auto; }
[data-style="glass"] .lv-scene-hero .lv-fin { border-radius: var(--radius-box); box-shadow: var(--shadow-3); }
.lv-scene-hero .lv-fin-t { margin-right: 4px; color: var(--color-text-primary); white-space: nowrap; }
.lv-scene-hero .lv-fin-b { display: inline-flex; flex-wrap: nowrap; gap: 8px; }

/* One view or two */
.lv-scene-hero .lv-twob { display: grid; place-items: center; width: 28px; height: 28px; padding: 0; flex: none; color: var(--color-text-secondary); cursor: pointer; border-color: var(--color-control-edge); }
.lv-scene-hero .lv-twob:hover { background: var(--color-control-hover); color: var(--color-text-primary); }
.lv-scene-hero .lv-twob[aria-pressed="true"] { color: var(--color-accent-text); box-shadow: inset 0 -2px 0 var(--color-accent); }

/* The turntable: eight stills at the picture's foot */
.lv-scene-hero .lv-tt { position: absolute; left: 50%; bottom: 72px; z-index: 14; transform: translateX(-50%); width: min(560px, calc(100% - 36px)); padding: 6px 8px 5px; display: flex; flex-direction: column; gap: 4px; }
[data-style="glass"] .lv-scene-hero .lv-tt { border-radius: var(--radius-box); }
.lv-scene-hero .lv-ttrow { display: grid; grid-template-columns: repeat(8, 1fr); gap: 4px; cursor: ew-resize; touch-action: none; outline: none; }
.lv-scene-hero .lv-ttrow:focus-visible { box-shadow: 0 0 0 2px var(--color-accent-text); }
.lv-scene-hero .lv-ttf { display: block; aspect-ratio: 16 / 9; overflow: hidden; background: var(--lv-stage); box-shadow: inset 0 0 0 1px var(--color-border); }
[data-style="glass"] .lv-scene-hero .lv-ttf { border-radius: min(var(--radius-field), 5px); }
.lv-scene-hero .lv-ttf[data-on] { box-shadow: inset 0 0 0 2px var(--color-accent-text); }
.lv-scene-hero .lv-ttf canvas { display: block; width: 100%; height: 100%; pointer-events: none; }
.lv-scene-hero .lv-ttnote { font-size: var(--type-sm); color: var(--color-text-secondary); text-align: center; }
.lv-scene-hero .lv-sstage:has(.lv-tt) .lv-tslot, .lv-scene-hero .lv-sstage:has(.lv-tt) .lv-fslot { bottom: 164px; }
`

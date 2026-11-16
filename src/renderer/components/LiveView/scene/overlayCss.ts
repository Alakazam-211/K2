// The overlay's styles, scoped under `.lv-scene-hero` (on the dark stage,
// so every token resolves dark in the light looks too): boxes, the ghost
// and arrows as thin strokes in the person's presence token (`--lv-c`,
// set per element), approximate and hidden boxes dashed, and the labels as
// plates on the stage (Glass rounds them, Terminal cuts them with the chip
// chamfer): the change under way tinted with its actor's colour, holds
// small and quiet, your pick filled with your colour, notes dashed. The
// settled change's tag and arrows fade, except under reduced motion.

export const OVERLAY_CSS = `
.lv-scene-hero .lv-ovl { position: absolute; z-index: 5; pointer-events: none; overflow: hidden; -webkit-mask-image: linear-gradient(to right, transparent, black 18px, black calc(100% - 18px), transparent), linear-gradient(to bottom, transparent, black 18px, black calc(100% - 18px), transparent); -webkit-mask-composite: source-in; mask-image: linear-gradient(to right, transparent, black 18px, black calc(100% - 18px), transparent), linear-gradient(to bottom, transparent, black 18px, black calc(100% - 18px), transparent); mask-composite: intersect; }
.lv-scene-hero .lv-bx { fill: none; stroke: var(--lv-c); stroke-width: 1.6; stroke-linejoin: round; stroke-linecap: round; vector-effect: non-scaling-stroke; }
.lv-scene-hero .lv-bx[data-style="sel"] { stroke-width: 1.1; opacity: .6; stroke-dasharray: 4 3; }
.lv-scene-hero .lv-bx[data-style="sel"][data-away] { opacity: .35; }
.lv-scene-hero .lv-bx[data-style="hold"] { stroke-width: 1.1; opacity: .6; }
.lv-scene-hero .lv-bx[data-style="mine"], .lv-scene-hero .lv-bx[data-style="live"] { stroke-width: 2.2; }
.lv-scene-hero .lv-bx[data-style="live"][data-rides] { stroke-width: 1.5; opacity: .8; }
.lv-scene-hero .lv-bx[data-style="settled"] { stroke-width: 1.3; opacity: .7; }
.lv-scene-hero .lv-bx[data-style="ghost"] { stroke-width: 1.2; stroke-dasharray: 3 4; opacity: .55; }
.lv-scene-hero .lv-bx[data-approx] { stroke-dasharray: 6 4; }
.lv-scene-hero .lv-bx[data-hidden] { stroke-dasharray: 2 4; opacity: .7; }
.lv-scene-hero .lv-dir { fill: none; stroke: var(--lv-c); stroke-width: 2.4; stroke-linecap: round; stroke-linejoin: round; vector-effect: non-scaling-stroke; opacity: .9; }
.lv-scene-hero .lv-dir[data-fading] { animation: lv-fadeout var(--motion-duration-slow) var(--motion-ease-standard) .15s forwards; }

/* Labels: placed in the stage's pixels by labels.ts */
.lv-scene-hero .lv-tags { position: absolute; inset: 0; z-index: 15; pointer-events: none; }
.lv-scene-hero .lv-tag, .lv-scene-hero .lv-offchip { position: absolute; display: inline-flex; align-items: center; gap: 5px; max-width: 46%; height: 24px; padding: 0 9px 0 7px; font-size: var(--type-md); font-weight: 500; line-height: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: var(--color-text-primary); border-color: var(--lv-c); }
.lv-scene-hero .lv-tag b { font-weight: 650; }
.lv-scene-hero .lv-tag .lv-dot, .lv-scene-hero .lv-offchip .lv-dot { display: inline-block; width: 8px; height: 8px; flex: none; background: var(--lv-c); }
[data-style="glass"] .lv-scene-hero .lv-tag .lv-dot, [data-style="glass"] .lv-scene-hero .lv-offchip .lv-dot { border-radius: 50%; }
.lv-scene-hero .lv-tag[data-kind="live"] { background: color-mix(in srgb, var(--lv-c) 24%, var(--lv-plate)); box-shadow: var(--shadow-2); }
.lv-scene-hero .lv-tag .lv-sub { margin-left: 2px; padding-left: 7px; border-left: 1px solid var(--color-border-strong); color: var(--color-text-secondary); }
.lv-scene-hero .lv-tag[data-kind="live"][data-fading] { animation: lv-fadeout .7s var(--motion-ease-standard) 1.8s forwards; }
.lv-scene-hero .lv-tag[data-kind="hold"] { height: 22px; gap: 4px; padding: 0 8px 0 6px; font-size: var(--type-sm); font-weight: 600; pointer-events: auto; cursor: default; border-color: color-mix(in srgb, var(--lv-c) 70%, transparent); }
.lv-scene-hero .lv-tag .lv-lk { display: grid; color: var(--lv-c); }
.lv-scene-hero .lv-tag .lv-q { font-weight: 450; color: var(--color-text-secondary); }
.lv-scene-hero .lv-tag[data-kind="mine"] { background: var(--lv-c); color: var(--color-bg); border-color: var(--lv-c); }
.lv-scene-hero .lv-tag[data-kind="sel"] { height: 22px; font-size: var(--type-sm); font-weight: 600; padding: 0 8px; }
.lv-scene-hero .lv-tag[data-kind="sel"][data-away] { border-style: dashed; color: var(--color-text-secondary); }
.lv-scene-hero .lv-tag[data-kind="note"] { height: 22px; font-size: var(--type-sm); color: var(--color-text-secondary); border-style: dashed; }
.lv-scene-hero .lv-offchip { height: 26px; gap: 6px; padding: 0 10px 0 8px; font: inherit; font-size: var(--type-md); font-weight: 600; cursor: pointer; pointer-events: auto; }
.lv-scene-hero .lv-offchip svg { color: var(--color-text-secondary); flex: none; }
.lv-scene-hero .lv-offchip:hover { background: var(--color-control-hover); }
.lv-scene-hero .lv-offchip:hover svg { color: var(--color-text-primary); }
@media (prefers-reduced-motion: reduce) {
  .lv-scene-hero .lv-tag[data-fading], .lv-scene-hero .lv-dir[data-fading] { animation: none; }
}
@keyframes lv-fadeout { to { opacity: 0; } }
`

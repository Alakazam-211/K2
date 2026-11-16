// Presence on a live text (build-editors.md E4b; the mock's ed.css): name
// flags and hairlines, holds (tint, bar, label, the line under your caret),
// the off-screen band, the Follow frame. Scoped under `.ed-tab`; every colour
// and radius is a token. Each person's colour is a presence slot
// (`.ed-pc-N` → `--pc`). Flag ink is the page colour: the presence tokens are
// light on the dark looks and deep on the light ones, so it reads in each.
// The kit's own caret names are hidden: the margin flags replace them.

import { SLOT_COLOURS } from '@/lib/editor/live/presence-slots'

const SLOTS = SLOT_COLOURS.map((c, i) => `.ed-tab .ed-pc-${i + 1} { --pc: ${c}; }`).join('\n')

export const PRESENCE_CSS = `
${SLOTS}
.ed-tab .cm-mpc-caret-name { display: none !important; }

.ed-tab .ed-flag-layer { pointer-events: none; }
.ed-tab .ed-flag { position: absolute; display: inline-flex; align-items: center; gap: 4px; box-sizing: border-box; height: 20px; padding: 0 7px 0 5px; border-radius: var(--radius-badge); background: var(--pc); color: var(--color-bg); font: 600 12px/20px var(--font-ui); white-space: nowrap; overflow: hidden; pointer-events: none; }
.ed-tab .ed-flag svg { flex: none; }
.ed-tab .ed-flag.ed-flag-mini { padding: 0; justify-content: center; }
.ed-tab .ed-caret-tag { position: absolute; display: inline-flex; align-items: center; gap: 3px; box-sizing: border-box; height: 18px; padding: 0 6px 0 4px; border-radius: var(--radius-badge); background: var(--pc); color: var(--color-bg); font: 600 11px/18px var(--font-ui); white-space: nowrap; overflow: hidden; pointer-events: none; opacity: 0; transition: opacity var(--motion-duration-fast) var(--motion-ease-standard); }
.ed-tab .ed-caret-tag svg { flex: none; }
.ed-tab .ed-caret-tag.ed-caret-tag-shown { opacity: 1; }
@media (prefers-reduced-motion: reduce) { .ed-tab .ed-caret-tag { transition: none; } }
.ed-tab .ed-fline { position: absolute; height: 0; border-top: 1px dashed var(--pc); opacity: 0.75; }

.ed-tab .ed-hold-layer .ed-hold-tint { background: color-mix(in srgb, var(--pc) 9%, transparent); border-radius: 0 var(--radius-box) var(--radius-box) 0; }
.ed-tab .ed-hold-layer .ed-hold-bar { background: var(--pc); }
html[data-style="terminal"] .ed-tab .ed-hold-layer .ed-hold-tint { border-radius: 0; }
.ed-tab .ed-hold-row { display: flex; justify-content: flex-end; padding: 6px 0 8px; }
.ed-tab .ed-hold-label { display: inline-flex; align-items: center; gap: 6px; height: 24px; box-sizing: border-box; padding: 0 3px 0 3px; border: 1.5px solid var(--pc); border-radius: var(--radius-pill); background: var(--color-bg-elevated); color: var(--color-text-primary); font: 500 12px/1 var(--font-ui); white-space: nowrap; }
.ed-tab .ed-hold-av { display: inline-grid; place-items: center; width: 18px; height: 18px; flex: none; border-radius: var(--radius-pill); background: var(--pc); color: var(--color-bg); font: 600 10px/1 var(--font-ui); }
.ed-tab .ed-hold-msg { display: inline-grid; place-items: center; width: 20px; height: 20px; padding: 0; border: 0; border-radius: var(--radius-pill); background: transparent; color: var(--color-text-secondary); cursor: pointer; }
.ed-tab .ed-hold-msg:hover { background: var(--color-control-hover-quiet); color: var(--color-text-primary); }
.ed-tab .ed-hold-msg:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 1px; }
html[data-style="terminal"] .ed-tab :is(.ed-hold-label, .ed-hold-msg, .ed-hold-hint) { border-radius: 0; }
.ed-tab .ed-hold-hint { display: flex; align-items: center; gap: 8px; margin: 6px 0 4px; padding: 6px 10px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-box); background: var(--color-bg-elevated); color: var(--color-text-primary); font: 12.5px/1.4 var(--ed-body, var(--font-ui)); white-space: normal; }
.ed-tab .ed-hold-hint svg { flex: none; color: var(--color-text-secondary); }
.ed-tab .ed-hold-hint b { font-weight: 600; }

.ed-tab .ed-live-slot[data-following] { position: relative; }
.ed-tab .ed-live-slot[data-following]::after { content: ''; position: absolute; inset: 0; z-index: 30; pointer-events: none; box-shadow: inset 0 0 0 2px var(--fc); }

.ed-tab .ed-band { position: relative; flex: none; height: 34px; background: var(--editor-bg, var(--color-bg)); }
.ed-tab .ed-band[data-side="top"] { border-bottom: 1px solid var(--color-border); }
.ed-tab .ed-band[data-side="bottom"] { border-top: 1px solid var(--color-border); }
.ed-tab .ed-band-chips { position: absolute; top: 5px; display: flex; gap: 6px; transform: translateX(-50%); max-width: calc(100% - 16px); }
.ed-tab .ed-edge { display: inline-flex; align-items: center; gap: 5px; height: 24px; padding: 0 8px 0 3px; border: 1px solid var(--color-control-edge); border-radius: var(--radius-pill); background: var(--color-bg-elevated); color: var(--color-text-secondary); font: 12px/1 var(--font-ui); white-space: nowrap; cursor: pointer; }
.ed-tab .ed-edge:hover { background: var(--color-control-hover); color: var(--color-text-primary); }
.ed-tab .ed-edge:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }
.ed-tab .ed-finished-btn { height: 26px; }
.ed-tab .ed-finished-btn.ed-finished-mark { height: 26px; width: 28px; padding: 0; justify-content: center; }
`

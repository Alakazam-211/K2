// The live editor's own styles (build-editors.md E4a), scoped under `.ed-tab`.
// Every colour and radius is a token. The kit's editor reads `--mpc-*`
// (LiveEditor/theme.ts), so the host sets those, never its selectors: Source
// and code in the code face, the frame's own focus ring (keyboard only), the
// writing surface's ground. Disconnected, others' carets and selections turn
// grey where they last were.

export const LIVE_CSS = `
.ed-tab .ed-live-slot { display: flex; flex-direction: column; }
.ed-tab .ed-live-host { flex: 1 1 auto; height: 100%; min-height: 0; --mpc-font: var(--font-code); --mpc-font-size: 13px; --mpc-line-height: 1.55; --mpc-focus: transparent; --mpc-bg: var(--editor-bg, var(--color-bg)); }
.ed-tab .ed-live-host > .mpc-editor { height: 100%; }
.ed-tab .ed-live-host .mpc-body { display: flex; flex-direction: column; }
.ed-tab .ed-live-host .mpc-body > .cm-editor { flex: 1 1 auto; min-height: 0; }
.ed-tab[data-offline] .cm-mpc-caret, .ed-tab[data-offline] .cm-mpc-caret-name, .ed-tab[data-offline] .cm-mpc-selection { --mpc-user: var(--color-text-muted) !important; --mpc-user-fg: var(--color-bg) !important; }
.ed-tab .ed-offline { color: var(--color-text-secondary); }
.ed-tab .ed-panel-tab { background: color-mix(in srgb, var(--color-text-primary) 6%, transparent); }
.ed-tab .ed-chip-btn { background: transparent; font-family: inherit; line-height: 1.4; }
.ed-tab .ed-chip-btn:hover { background: var(--color-control-hover-quiet); }
.ed-tab .ed-chip-btn[aria-disabled] { color: var(--color-text-secondary); cursor: default; background: transparent; }
.ed-tab .ed-chip-btn:focus-visible { outline: 2px solid var(--color-accent-text); outline-offset: 2px; }
.ed-tab .lv-change[data-undone] [data-lv-change-text] { color: var(--color-text-secondary); }
.ed-tab .lv-link[aria-disabled] { color: var(--color-text-secondary); text-decoration: none; cursor: default; }
.ed-tab .ed-panel-foot { background: var(--editor-bg, var(--color-bg)); }
`

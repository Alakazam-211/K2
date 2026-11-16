// The live shell's own styles, scoped under `.lv-shell`: the state dot, the
// avatar stack's overlap (a hovered or followed avatar's ring is a coloured
// edge with the background inside it, so a light presence fill never runs
// into it), the follow tint, rows, the notice tones and the
// stage placeholder. Every colour, radius and duration is a token (the Style
// package); a person's colour comes from the data as `--lv-c`. Reduced
// motion turns every animation off. Rendered once per tab as a <style>.

export const LIVE_SHELL_CSS = `
.lv-shell { --lv-caption: var(--type-xs); background: var(--color-bg); }
.lv-shell .lv-num { font-variant-numeric: tabular-nums; }
.lv-shell .lv-dot { width: 6px; height: 6px; flex: none; border-radius: 50%; background: currentColor; }
.lv-shell .lv-dot[data-pulse] { animation: lv-pulse 1.4s var(--motion-ease-standard) infinite; }
.lv-shell .lv-caption { font-size: var(--lv-caption); font-weight: 500; color: var(--color-text-secondary); white-space: nowrap; }
.lv-shell .lv-file-glyph { background: color-mix(in srgb, var(--color-accent) 14%, transparent); color: var(--color-accent-text); }
.lv-shell .lv-icon-btn:hover { background: var(--color-control-hover-quiet); color: var(--color-text-primary); }
.lv-shell .lv-icon-btn[aria-pressed="true"] { background: var(--color-control-hover-quiet); color: var(--color-text-primary); }
.lv-shell .lv-avatar[data-away] { opacity: .45; }
.lv-shell .lv-avatar[data-grey] { filter: grayscale(1); opacity: .7; }
.lv-shell .lv-stack [data-lv-stack-item] + [data-lv-stack-item] { margin-left: -8px; }
.lv-shell .lv-ring, .lv-shell .lv-ring-gap { background: var(--color-bg); }
.lv-shell .lv-head .lv-ring, .lv-shell .lv-head .lv-ring-gap { background: var(--color-tab-active); }
.lv-shell .lv-stack button[data-lv-stack-item]:hover .lv-avatar { filter: brightness(1.15); }
.lv-shell .lv-stack button[data-lv-stack-item]:hover .lv-ring { background: var(--color-text-secondary); }
.lv-shell .lv-stack button[aria-pressed="true"] .lv-ring { background: var(--color-accent-text); }
.lv-shell .lv-following { background: color-mix(in srgb, var(--lv-c, var(--color-accent)) 16%, var(--color-bg-elevated)); }
.lv-shell .lv-following:hover { background: color-mix(in srgb, var(--lv-c, var(--color-accent)) 28%, var(--color-bg-elevated)); }
.lv-shell .lv-following-dot { width: 8px; height: 8px; flex: none; border-radius: 50%; background: var(--lv-c, var(--color-accent)); }
.lv-shell .lv-row-btn { background: transparent; border: 0; font: inherit; color: inherit; }
.lv-shell .lv-row-btn:hover { background: var(--color-row-hover); }
.lv-shell .lv-row-btn:hover .lv-status { color: var(--color-text-primary); }
.lv-shell .lv-row-on { background: color-mix(in srgb, var(--lv-c, var(--color-accent)) 12%, transparent); }
.lv-shell .lv-away .lv-person { color: var(--color-text-secondary); }
.lv-shell .lv-change-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--lv-c, var(--color-text-muted)); }
.lv-shell .lv-link { border: 0; background: none; padding: 0; font: inherit; color: var(--color-text-secondary); text-decoration: underline; text-underline-offset: 2px; }
.lv-shell .lv-link:hover { color: var(--color-text-primary); text-decoration-thickness: 2px; }
.lv-shell .lv-notice { --lv-t: var(--color-accent); border-color: color-mix(in srgb, var(--lv-t) 45%, var(--color-control-edge)); background: color-mix(in srgb, var(--lv-t) 6%, var(--color-bg)); }
.lv-shell .lv-notice[data-tone="warn"] { --lv-t: var(--color-status-warn); }
.lv-shell .lv-notice[data-tone="bad"] { --lv-t: var(--color-status-error); }
.lv-shell .lv-notice-icon { color: var(--lv-t); }
.lv-shell .lv-notice[data-tone="info"] .lv-notice-icon { color: var(--color-accent-text); }
.lv-shell .lv-notice[data-tone="warn"] .lv-notice-icon { color: var(--color-status-warn-text); }
.lv-shell .lv-notice[data-tone="bad"] .lv-notice-icon, .lv-shell .lv-bad-ink { color: var(--color-status-error-text); }
.lv-shell .lv-edge-avs { gap: 2px; }
.lv-shell .lv-edge:hover { background: var(--color-control-hover); color: var(--color-text-primary); }
.lv-shell .lv-stage { background: var(--color-bg-inset); border: 1px solid var(--color-border); }
.lv-shell .lv-panel { background: var(--color-bg); border-left: 1px solid var(--color-border); }
.lv-shell .lv-panel[data-sheet] { box-shadow: var(--ring-surface); }
.lv-shell .lv-say { border-top: 1px solid var(--color-border); }
/* The header gives things up in order while it would run past the tab (LiveHeader). */
.lv-shell .lv-head:is([data-tight="1"], [data-tight="2"], [data-tight="3"], [data-tight="4"], [data-tight="5"], [data-tight="6"]) [data-lv-give="1"] { display: none; }
.lv-shell .lv-head:is([data-tight="2"], [data-tight="3"], [data-tight="4"], [data-tight="5"], [data-tight="6"]) [data-testid="lv-stack"] { display: none; }
.lv-shell .lv-head:is([data-tight="3"], [data-tight="4"], [data-tight="5"], [data-tight="6"]) [data-variant="following"] .lv-follow-words { display: none; }
.lv-shell .lv-head:is([data-tight="4"], [data-tight="5"], [data-tight="6"]) [data-lv-give="4"] { display: none; }
@keyframes lv-pulse { 50% { opacity: .35; } }
@media (prefers-reduced-motion: reduce) { .lv-shell *, .lv-shell *::before, .lv-shell *::after { animation: none !important; transition: none !important; } }
`

// The save model's own styles (build-editors.md E1c), scoped under `.ed-tab`:
// the status line's failed word, the kept pair's marks and tags, and Compare.
// Every colour and radius is a token.

export const SAVE_CSS = `
.ed-tab .ed-bad { color: var(--color-status-error-text); }
.ed-tab .ed-si [data-tone="bad"] { color: var(--color-text-primary); }
.ed-tab .ed-save-notice p > b + span { margin-top: 1px; }
/* At 800 the actions wrap below the words; a lone × stays on the headline's row. */
.ed-tab[data-narrow] .ed-save-actions:not([data-close-only]), .ed-tab .lv-shell[data-narrow] .ed-save-actions:not([data-close-only]) { flex-basis: 100%; justify-content: flex-end; }

/* The kept pair: a warn tint, a bar in the left margin, a tag in the right. */
.ed-tab .cm-line.ed-kept { position: relative; background: color-mix(in srgb, var(--color-status-warn) 11%, transparent); }
.ed-tab .cm-line.ed-kept::before { content: ""; position: absolute; left: -14px; top: 0; bottom: 0; width: 3px; background: var(--color-status-warn); border-radius: 2px; }
.ed-tab .cm-line.ed-kept[data-kept="theirs"]::before { opacity: .6; }
.ed-tab .ed-kept-tag { position: absolute; left: calc(100% + 18px); top: 3px; display: inline-flex; align-items: center; height: 20px; padding: 0 8px; white-space: nowrap; font-family: var(--font-ui); font-size: 11.5px; font-weight: 500; line-height: 1; color: var(--color-text-primary); background: var(--editor-bg, var(--color-bg)); border: 1px solid var(--color-status-warn); border-radius: var(--radius-pill); pointer-events: none; }
/* No right margin (Source, code, a narrow tab): the tag sits on its own row
   above the side's first line, at the right, so it never covers text and the
   line numbers stay with their lines. */
.ed-tab .ed-kept-row { display: none; }
.ed-tab:is([data-mode="source"], [data-kind="code"], [data-kept-row]) .ed-kept-row, .ed-tab .lv-shell[data-narrow] .ed-kept-row { display: flex; justify-content: flex-end; padding: 4px 6px 2px; background: color-mix(in srgb, var(--color-status-warn) 11%, transparent); }
.ed-tab .ed-kept-row .ed-kept-tag { position: static; }
.ed-tab:is([data-mode="source"], [data-kind="code"], [data-kept-row]) .cm-line > .ed-kept-tag, .ed-tab .lv-shell[data-narrow] .cm-line > .ed-kept-tag { display: none; }
.ed-tab:is([data-mode="source"], [data-kind="code"]) .cm-line.ed-kept::before { left: 0; }

/* Compare: before against now, read only, on the diff tokens. */
.ed-tab .ed-compare-heads { display: grid; grid-template-columns: 1fr 1fr; flex: none; border-bottom: 1px solid var(--color-border); font-family: var(--font-ui); font-size: 12px; color: var(--color-text-secondary); }
.ed-tab .ed-compare[data-narrow] .ed-compare-heads { grid-template-columns: 1fr; }
.ed-tab .ed-compare-head { padding: 8px 14px; }
.ed-tab .ed-compare-head + .ed-compare-head { border-left: 1px solid var(--color-border); }
.ed-tab .ed-compare[data-narrow] .ed-compare-head + .ed-compare-head { border-left: 0; padding-top: 0; }
.ed-tab .ed-compare-head b { color: var(--color-text-primary); font-weight: 600; }
.ed-tab .ed-compare-head[data-side="before"] b::first-letter { color: var(--color-diff-remove-text); }
.ed-tab .ed-compare-head[data-side="now"] b::first-letter { color: var(--color-diff-add-text); }
.ed-tab .ed-compare-body .cm-editor { font-family: var(--font-code); font-size: 12.5px; }
.ed-tab .ed-compare-body .cm-mergeView, .ed-tab .ed-compare-body > .cm-editor { min-height: 100%; }
.ed-tab .ed-compare-body .cm-merge-a .cm-changedLine, .ed-tab .ed-compare-body .cm-deletedChunk { background: color-mix(in srgb, var(--color-diff-remove-text) 13%, transparent); }
.ed-tab .ed-compare-body .cm-merge-b .cm-changedLine, .ed-tab .ed-compare-body .cm-inlineChangedLine { background: color-mix(in srgb, var(--color-diff-add-text) 13%, transparent); }
.ed-tab .ed-compare-body .cm-merge-a .cm-changedText, .ed-tab .ed-compare-body .cm-deletedChunk .cm-deletedText { background: color-mix(in srgb, var(--color-diff-remove-text) 30%, transparent); }
.ed-tab .ed-compare-body .cm-merge-b .cm-changedText { background: color-mix(in srgb, var(--color-diff-add-text) 30%, transparent); }
.ed-tab .ed-compare-body .cm-changeGutter { width: 3px; padding: 0; }
.ed-tab .ed-compare-body .cm-merge-a .cm-changedLineGutter { background: var(--color-diff-remove-text); }
.ed-tab .ed-compare-body .cm-merge-b .cm-changedLineGutter, .ed-tab .ed-compare-body .cm-changedLineGutter { background: var(--color-diff-add-text); }
.ed-tab .ed-compare-body .cm-merge-a { border-right: 1px solid var(--color-border); }
.ed-tab .ed-compare-body .cm-collapsedLines { background: var(--color-bg-inset); color: var(--color-text-secondary); font-family: var(--font-ui); font-size: 12px; padding: 4px 14px; text-decoration: underline; text-underline-offset: 2px; cursor: pointer; }
.ed-tab .ed-compare-body .cm-collapsedLines:hover { color: var(--color-text-primary); }
.ed-tab .ed-compare-body .cm-collapsedLines::before, .ed-tab .ed-compare-body .cm-collapsedLines::after { content: none; }
`

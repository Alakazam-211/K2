// K2's own editor code (see ./index.ts); started from k2-mpc-client 0.11.0's editor/theme.ts (deleted from the kit in v0.10.2).
// The editor's default look: every value comes from a CSS variable on `.mpc-editor`, so a host
// themes it without overriding selectors. `--mpc-user` (and `--mpc-user-fg`, a readable text
// colour on it) is set inline per participant.

import { EditorView } from "@codemirror/view";

/** The variables and their defaults (the host may set any of them on `.mpc-editor`). */
// K2 maps every colour to the look's tokens (build-editors.md E1a), so the live editor
// paints like CodeEditor in each look, never the browser's `Canvas` white.
export const CSS_VARS = {
  "--mpc-bg": "var(--editor-bg, var(--color-bg))",
  "--mpc-fg": "var(--color-text-primary)",
  "--mpc-font": "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
  "--mpc-font-size": "14px",
  "--mpc-line-height": "1.5",
  "--mpc-caret-width": "2px",
  "--mpc-label-fg": "",
  "--mpc-selection-alpha": "0.25",
  "--mpc-rejected": "var(--color-status-error-text)",
  "--mpc-banner-bg": "var(--color-bg-elevated)",
  "--mpc-banner-fg": "var(--color-text-primary)",
  "--mpc-focus": "var(--color-accent-text)",
} as const;

/** Prefix every selector with `&.mpc-cm` (each part of a selector list). */
type ThemeSpec = Parameters<typeof EditorView.baseTheme>[0];
function scoped(spec: ThemeSpec): ThemeSpec {
  const scope = (sel: string): string => (sel.startsWith("&") ? `&.mpc-cm${sel.slice(1)}` : `&.mpc-cm ${sel}`);
  return Object.fromEntries(Object.entries(spec).map(([k, val]) => [k.split(/,\s*/).map(scope).join(", "), val]));
}

const v = (name: keyof typeof CSS_VARS) => (CSS_VARS[name] ? `var(${name}, ${CSS_VARS[name]})` : `var(${name})`);

/** Styles inside the editor. Every rule is scoped to `.mpc-cm` (the live editor's own class):
 * a base theme's rules reach every CodeMirror in the window, so unscoped they outlived the
 * live view and painted the editor that replaced it (P12). */
export const LIVE_BASE_THEME_SPEC = scoped({
  "&": {
    backgroundColor: v("--mpc-bg"),
    color: v("--mpc-fg"),
    fontSize: v("--mpc-font-size"),
    height: "100%",
  },
  "&.cm-focused": { outline: `2px solid ${v("--mpc-focus")}`, outlineOffset: "-2px" },
  ".cm-scroller": { fontFamily: v("--mpc-font"), lineHeight: v("--mpc-line-height") },
  // room above the first line, so nothing drawn there is cut off
  ".cm-content": { paddingTop: "0.4em", paddingBottom: "0.4em" },
  ".cm-mpc-caret": {
    position: "relative",
    borderLeft: `${v("--mpc-caret-width")} solid var(--mpc-user, currentColor)`,
    marginLeft: "-1px",
    marginRight: "-1px",
    pointerEvents: "none",
  },
  ".cm-mpc-caret-name": {
    position: "absolute",
    bottom: "100%",
    left: "-2px",
    zIndex: "10",
    fontFamily: "system-ui, sans-serif",
    fontSize: "0.75em",
    lineHeight: "1.3",
    padding: "0 4px",
    backgroundColor: "var(--mpc-user, currentColor)",
    color: "var(--mpc-label-fg, var(--mpc-user-fg, #fff))",
    whiteSpace: "nowrap",
    borderRadius: "var(--radius-pill)",
    pointerEvents: "none",
    userSelect: "none",
    opacity: "0",
    transition: "opacity 120ms",
  },
  ".cm-mpc-caret-name-below": { bottom: "auto", top: "100%" },
  // this person's own caret in another tab or on another device: a dashed line, its name in
  // italics ("You · other tab"), its selection fainter
  ".cm-mpc-caret-same": { borderLeftStyle: "dashed" },
  ".cm-mpc-caret-same .cm-mpc-caret-name": { fontStyle: "italic" },
  ".cm-mpc-selection-same": {
    backgroundColor: `color-mix(in srgb, var(--mpc-user) calc(${v("--mpc-selection-alpha")} * 50%), transparent)`,
  },
  // someone away (the roster's `away`): their caret and selection dimmed
  ".cm-mpc-caret-away": { opacity: "0.5" },
  ".cm-mpc-selection-away": {
    backgroundColor: `color-mix(in srgb, var(--mpc-user) calc(${v("--mpc-selection-alpha")} * 50%), transparent)`,
  },
  ".cm-mpc-caret-name-shown": { opacity: "1" },
  "&.mpc-labels-always .cm-mpc-caret-name": { opacity: "1" },
  "&.mpc-labels-hover .cm-mpc-caret:hover .cm-mpc-caret-name, &.mpc-labels-hover .cm-line:hover .cm-mpc-caret-name": {
    opacity: "1",
  },
  ".cm-mpc-selection": {
    backgroundColor: `color-mix(in srgb, var(--mpc-user) calc(${v("--mpc-selection-alpha")} * 100%), transparent)`,
  },
  ".cm-mpc-rejected": {
    textDecoration: `underline wavy ${v("--mpc-rejected")}`,
    textDecorationSkipInk: "none",
  },
  ".cm-mpc-rejected-text": { opacity: "0.6", textDecoration: "line-through", color: v("--mpc-rejected") },
  ".cm-mpc-insert-preview": { outline: `1px dashed ${v("--mpc-rejected")}`, color: v("--mpc-rejected"), opacity: "0.8" },
  ".cm-mpc-reveal": { backgroundColor: `color-mix(in srgb, ${v("--mpc-focus")} 15%, transparent)` },
});

export const editorTheme = EditorView.baseTheme(LIVE_BASE_THEME_SPEC);

/** Styles outside CodeMirror (the frame, the banner, the roster, the rejections list). */
export const FRAME_CSS = `
html[data-scheme="dark"] .mpc-editor { color-scheme: dark; }
html[data-scheme="light"] .mpc-editor { color-scheme: light; }
.mpc-editor { display: flex; flex-direction: column; min-height: 0; background: ${v("--mpc-bg")}; color: ${v("--mpc-fg")}; font: 13px system-ui, sans-serif; }
.mpc-editor .mpc-body { flex: 1 1 auto; min-height: 0; position: relative; }
.mpc-editor .mpc-banner { padding: 6px 10px; background: ${v("--mpc-banner-bg")}; color: ${v("--mpc-banner-fg")}; }
.mpc-editor .mpc-banner[hidden] { display: none; }
.mpc-editor .mpc-banner button { margin-left: 8px; }
.mpc-editor .mpc-roster { display: flex; flex-wrap: wrap; gap: 4px; padding: 4px 6px; }
.mpc-editor .mpc-roster[hidden] { display: none; }
.mpc-editor .mpc-roster button { border: 0; border-radius: 10px; padding: 1px 8px; background: var(--mpc-user); color: var(--mpc-label-fg, var(--mpc-user-fg)); font: inherit; cursor: pointer; }
.mpc-editor .mpc-roster button[data-away="true"] { opacity: 0.5; }
.mpc-editor .mpc-roster button:focus-visible, .mpc-editor .mpc-rejections button:focus-visible, .mpc-editor .mpc-banner button:focus-visible { outline: 2px solid ${v("--mpc-focus")}; outline-offset: 1px; }
.mpc-editor .mpc-rejections { margin: 0; padding: 4px 6px; list-style: none; border-top: 1px solid ${v("--mpc-rejected")}; }
.mpc-editor .mpc-rejections[hidden] { display: none; }
.mpc-editor .mpc-rejections li { display: flex; gap: 6px; align-items: baseline; padding: 2px 0; }
.mpc-editor .mpc-rejections .mpc-may-have-applied { font-style: italic; }
.mpc-editor .mpc-note { padding: 2px 6px; }
.mpc-editor .mpc-note[hidden] { display: none; }
.mpc-editor .mpc-rejections .mpc-rejected-snippet { color: ${v("--mpc-rejected")}; white-space: pre; overflow: hidden; text-overflow: ellipsis; max-width: 40ch; font-family: ${v("--mpc-font")}; user-select: text; -webkit-user-select: text; }
`;

let injected = new WeakSet<Document>();

/** Put the frame styles in `doc` once (CodeMirror's own styles go through its style module). */
export function injectFrameStyles(doc: Document): void {
  if (injected.has(doc)) return;
  injected.add(doc);
  const el = doc.createElement("style");
  el.setAttribute("data-mpc-editor", "");
  el.textContent = FRAME_CSS;
  doc.head.appendChild(el);
}

/** For tests: forget which documents have the styles. */
export function resetInjectedStyles(): void {
  injected = new WeakSet<Document>();
}

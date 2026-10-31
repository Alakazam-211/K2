// The editor's default look: every value comes from a CSS variable on `.mpc-editor`, so a host
// themes it without overriding selectors. `--mpc-user` (and `--mpc-user-fg`, a readable text
// colour on it) is set inline per participant.

import { EditorView } from "@codemirror/view";

/** The variables and their defaults (the host may set any of them on `.mpc-editor`). */
export const CSS_VARS = {
  "--mpc-bg": "Canvas",
  "--mpc-fg": "CanvasText",
  "--mpc-font": "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
  "--mpc-font-size": "14px",
  "--mpc-line-height": "1.5",
  "--mpc-caret-width": "2px",
  "--mpc-label-fg": "",
  "--mpc-selection-alpha": "0.25",
  "--mpc-rejected": "#c62828",
  "--mpc-banner-bg": "#fff4e5",
  "--mpc-banner-fg": "#5f3b00",
  "--mpc-focus": "#1a73e8",
} as const;

const v = (name: keyof typeof CSS_VARS) => (CSS_VARS[name] ? `var(${name}, ${CSS_VARS[name]})` : `var(${name})`);

/** Styles inside the editor (CodeMirror's scoped base theme). */
export const editorTheme = EditorView.baseTheme({
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
    borderRadius: "3px",
    pointerEvents: "none",
    userSelect: "none",
    opacity: "0",
    transition: "opacity 120ms",
  },
  ".cm-mpc-caret-name-below": { bottom: "auto", top: "100%" },
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
  ".cm-mpc-reveal": { backgroundColor: `color-mix(in srgb, ${v("--mpc-focus")} 15%, transparent)` },
});

/** Styles outside CodeMirror (the frame, the banner, the roster, the rejections list). */
export const FRAME_CSS = `
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
.mpc-editor .mpc-rejections .mpc-rejected-snippet { color: ${v("--mpc-rejected")}; white-space: pre; overflow: hidden; text-overflow: ellipsis; max-width: 40ch; font-family: ${v("--mpc-font")}; }
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

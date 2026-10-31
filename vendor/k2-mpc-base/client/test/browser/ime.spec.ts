// IME compositions with remote changes arriving in the middle: the person's composed text must
// be neither lost nor doubled, and every editor and the server must end with the same text.
//
// Two drivers:
// - `synthetic` (every browser): composition events, with the composing text written into the
//   editor's DOM at the caret as a browser does under an IME. It runs on WebKit, which has no
//   IME automation.
// - `cdp` (Chromium only): Chromium's own IME input (`Input.imeSetComposition`, then
//   `Input.insertText` to commit), the closest to a real input method that automation offers.
//
// The caveat, for WebKit: Linux WebKit is WebKitGTK, not the macOS system web view (WKWebView),
// and macOS input methods (Japanese, Chinese) compose differently. A pass here is an early
// signal only; the real IME check is the host application's, on macOS, once the component is
// mounted there.
//
// One line per case goes to the log: `IME <browser> <driver> <case>: OK ...`.

import { expect, test, type Page } from "@playwright/test";
import { HARNESS } from "./global-setup";

/** A remote change: on the server's text as it is then, before the first occurrence of `at`
 * (`^`: the start, `$`: the end), deleting `del` characters there and inserting `insert`; or
 * another editor typing. */
type Remote = { kind: "server"; at: string; del?: number; insert: string } | { kind: "type"; editor: number; pos: number; text: string };

interface Case {
  name: string;
  /** The remote changes: one after the first composition update, one after the second. */
  remotes: [Remote, Remote];
  /** Text each remote change leaves in the final document. */
  expectIn: string[];
  /** Text a remote deletion takes out. */
  expectOut?: string[];
}

const INITIAL = "alpha beta\ngamma delta\n";
const CARET = 10; // after "beta" (ASCII: bytes = UTF-16 here)

const CASES: Case[] = [
  {
    name: "remote insert before the caret",
    remotes: [
      { kind: "server", at: "^", insert: "X1 " },
      { kind: "server", at: "^", insert: "X2 " },
    ],
    expectIn: ["X2 X1 alpha beta"],
  },
  {
    name: "remote insert after the caret",
    remotes: [
      { kind: "server", at: "$", insert: "Z1" },
      { kind: "server", at: "gamma", insert: "Z2 " },
    ],
    expectIn: ["Z2 gamma delta", "Z1"],
  },
  {
    name: "remote delete before the caret",
    remotes: [
      { kind: "server", at: "alpha", del: 6, insert: "" },
      { kind: "server", at: "beta", del: 1, insert: "B" },
    ],
    expectIn: ["Beta"],
    expectOut: ["alpha"],
  },
  {
    // first where the caret is (the server hasn't the composing text yet), then right at the
    // composing text's start on the server
    name: "remote insert at the caret",
    remotes: [
      { kind: "server", at: "\ngamma", insert: "@1" },
      { kind: "server", at: "に", insert: "@2" },
    ],
    expectIn: ["@1", "@2"],
  },
  {
    name: "another editor types on the same line",
    remotes: [
      { kind: "type", editor: 1, pos: 0, text: "B: " },
      { kind: "type", editor: 1, pos: 3, text: "hi " },
    ],
    expectIn: ["B: hi alpha beta"],
  },
];

const COMPOSED = "日本";

async function load(page: Page) {
  await page.setContent("<!doctype html><html><head><meta charset=utf-8></head><body></body></html>");
  await page.addScriptTag({ path: HARNESS });
  await page.evaluate(async (initial) => {
    const w = window as any;
    w.sc = w.MPC.scenario(initial, 2);
    await w.sc.settle();
  }, INITIAL);
  const st = await state(page);
  expect(st.states).toEqual(["live", "live"]);
  expect(st.docs).toEqual([INITIAL, INITIAL]);
}

async function state(page: Page) {
  return page.evaluate(() => (window as any).sc.state()) as Promise<{
    server: string;
    docs: string[];
    texts: string[];
    versions: number[];
    serverVersion: number;
    states: string[];
  }>;
}

async function focusAt(page: Page, pos: number) {
  await page.evaluate((p) => {
    const v = (window as any).sc.editors[0].view;
    v.focus();
    v.dispatch({ selection: { anchor: p } });
  }, pos);
}

/** Make a remote change and deliver it to every editor, mid-composition. Returns whether
 * editor 0's document left it out for now (held under the composition). */
async function remote(page: Page, r: Remote): Promise<boolean> {
  return page.evaluate(async (r) => {
    const sc = (window as any).sc;
    if (r.kind === "server") {
      const text: string = sc.server.text;
      const i = r.at === "^" ? 0 : r.at === "$" ? text.length : text.indexOf(r.at);
      if (i < 0) throw new Error(`no ${JSON.stringify(r.at)} in the server's text ${JSON.stringify(text)}`);
      const bytes = (s: string) => new TextEncoder().encode(s).length;
      const start = bytes(text.slice(0, i));
      const end = start + bytes(text.slice(i, i + (r.del ?? 0)));
      sc.serverEdit([{ start, end, insert: r.insert }]);
    }
    else sc.type(r.editor, r.pos, r.text);
    await sc.settle();
    const st = sc.state();
    return st.docs[0] !== st.texts[0];
  }, r);
}

async function finish(page: Page, c: Case, browser: string, driver: string, held: boolean[]) {
  await page.waitForTimeout(150); // compositionend's flush runs on a timer
  await page.evaluate(async () => (window as any).sc.settle());
  const st = await state(page);
  const report = await page.evaluate(() => {
    const sc = (window as any).sc;
    return { errors: sc.errors as string[], rejected: sc.rejected as unknown[], reloads: sc.reloads as string[] };
  });
  const line = `IME ${browser} ${driver} ${c.name}`;
  const count = st.server.split(COMPOSED).length - 1;
  const problems: string[] = [];
  if (count !== 1) problems.push(`the composed text is in the server's text ${count} times`);
  if (st.server.includes("に") || st.server.includes("ほ")) problems.push("a composing step was left in the text");
  if (st.docs[0] !== st.server || st.texts[0] !== st.server) problems.push("editor 0 differs from the server");
  if (st.docs[1] !== st.server || st.texts[1] !== st.server) problems.push("editor 1 differs from the server");
  if (st.versions.some((v) => v !== st.serverVersion)) problems.push(`versions ${st.versions} vs ${st.serverVersion}`);
  for (const s of c.expectIn) if (!st.server.includes(s)) problems.push(`missing ${JSON.stringify(s)}`);
  for (const s of c.expectOut ?? []) if (st.server.includes(s)) problems.push(`still has ${JSON.stringify(s)}`);
  if (report.errors.length) problems.push(`errors: ${report.errors.join("; ")}`);
  if (report.rejected.length) problems.push(`rejected: ${JSON.stringify(report.rejected)}`);
  if (report.reloads.length) problems.push(`reloads: ${report.reloads.join("; ")}`);
  console.log(
    `${line}: ${problems.length ? "FAIL " + problems.join(" | ") : "OK"} held=${held.join(",")} server=${JSON.stringify(st.server)}`,
  );
  expect(problems, `${line}; docs ${JSON.stringify(st.docs)}`).toEqual([]);
}

for (const c of CASES) {
  test(`synthetic composition: ${c.name}`, async ({ page, browserName }) => {
    await load(page);
    await focusAt(page, CARET);
    const held: boolean[] = [];
    await page.evaluate(() => {
      const w = window as any;
      w.comp = w.MPC.syntheticComposition(w.sc.editors[0].view);
      w.comp.start();
      w.comp.update("に");
    });
    await page.waitForTimeout(30);
    held.push(await remote(page, c.remotes[0]));
    await page.evaluate(() => (window as any).comp.update("にほ"));
    await page.waitForTimeout(30);
    held.push(await remote(page, c.remotes[1]));
    await page.evaluate((t) => (window as any).comp.update(t), COMPOSED);
    await page.waitForTimeout(30);
    const intact = await page.evaluate((t) => (window as any).comp.intact(t), COMPOSED);
    expect(intact, "the composing text was replaced under the composition").toBe(true);
    await page.evaluate((t) => (window as any).comp.end(t), COMPOSED);
    await finish(page, c, browserName, "synthetic", held);
  });

  test(`IME composition (Chromium input): ${c.name}`, async ({ page, browserName }) => {
    test.skip(browserName !== "chromium", "Chromium's IME input is driven over its own protocol");
    await load(page);
    await focusAt(page, CARET);
    const cdp = await page.context().newCDPSession(page);
    const held: boolean[] = [];
    await cdp.send("Input.imeSetComposition", { text: "に", selectionStart: 1, selectionEnd: 1 });
    await page.waitForTimeout(30);
    held.push(await remote(page, c.remotes[0]));
    await cdp.send("Input.imeSetComposition", { text: "にほ", selectionStart: 2, selectionEnd: 2 });
    await page.waitForTimeout(30);
    held.push(await remote(page, c.remotes[1]));
    await cdp.send("Input.imeSetComposition", { text: COMPOSED, selectionStart: 2, selectionEnd: 2 });
    await page.waitForTimeout(30);
    await cdp.send("Input.insertText", { text: COMPOSED });
    await finish(page, c, browserName, "cdp", held);
  });
}

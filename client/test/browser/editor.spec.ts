// The live editor in a real browser, against the fake server: typing from the keyboard in two
// editors, remote caret labels (readable, never cut off on line 1), a rejection's actions from
// the keyboard, and a dropped socket with the banner and the resume.

import { expect, test, type Page } from "@playwright/test";
import { HARNESS } from "./global-setup";

async function load(page: Page, initial: string, labels: "always" | "active" | "hover" = "always") {
  await page.setContent("<!doctype html><html><head><meta charset=utf-8></head><body></body></html>");
  await page.addScriptTag({ path: HARNESS });
  await page.evaluate(
    async ([initial, labels]) => {
      const w = window as any;
      w.sc = w.MPC.scenario(initial, 2, { labels });
      await w.sc.settle();
    },
    [initial, labels] as const,
  );
}

const state = (page: Page) => page.evaluate(() => (window as any).sc.state());
const settle = (page: Page) => page.evaluate(() => (window as any).sc.settle());

/** Settle until every editor is live again (the reconnect timers are real here). */
async function settleLive(page: Page) {
  await page.evaluate(async () => {
    const sc = (window as any).sc;
    for (let i = 0; i < 100; i++) {
      await sc.settle();
      if (sc.editors.every((e: any) => e.state === "live" && e.pending === 0)) return;
      await new Promise((r) => setTimeout(r, 20));
    }
    throw new Error(`not live: ${sc.editors.map((e: any) => e.state)}`);
  });
}

async function noProblems(page: Page) {
  const p = await page.evaluate(() => {
    const sc = (window as any).sc;
    return { errors: sc.errors, reloads: sc.reloads };
  });
  expect(p.errors).toEqual([]);
  expect(p.reloads).toEqual([]);
}

test("two editors typing from the keyboard converge", async ({ page }) => {
  await load(page, "# Title\n\nbody\n");
  await page.click("#ed0 .cm-content");
  await page.keyboard.press("Control+End");
  await page.keyboard.type("one 日本 🎉");
  await settle(page);
  await page.click("#ed1 .cm-line >> nth=0");
  await page.keyboard.press("Home");
  await page.keyboard.type("> ");
  await page.keyboard.press("Enter");
  await settle(page);
  await page.click("#ed0 .cm-content");
  await page.keyboard.press("Control+Home");
  await page.keyboard.type("A");
  await page.keyboard.press("Backspace");
  await page.keyboard.type("Z");
  await settle(page);
  const st = await state(page);
  expect(st.docs[0]).toBe(st.server);
  expect(st.docs[1]).toBe(st.server);
  expect(st.server).toContain("one 日本 🎉");
  expect(st.server).toContain("> ");
  expect(st.server.startsWith("Z")).toBe(true);
  await noProblems(page);
});

test("remote caret labels are readable and not cut off on line 1", async ({ page }) => {
  await load(page, "first line\nsecond line\nthird\n");
  const colours = ["#ffeb3b", "#7c3aed", "#e11d48", "#16a34a", "#0ea5e9", "#f97316", "#111111", "#fafafa"];
  await page.evaluate(async (colours) => {
    const sc = (window as any).sc;
    const conn = sc.transports[0].conn;
    const v = sc.server.version;
    sc.server.sendTo(conn, {
      type: "roster_event",
      event: {
        kind: "snapshot",
        version: v,
        members: colours.map((c: string, i: number) => ({
          handle: `x${i}`,
          name: `Person ${i}`,
          colour: c,
          kind: "person",
          role: "editor",
          away: false,
          // half on line 1, half on line 2
          cursor: { root: "source", anchor: i % 2 ? 12 + i : i, head: i % 2 ? 12 + i : i, version: v },
        })),
      },
    });
    await sc.settle();
  }, colours);
  await page.waitForTimeout(200);
  const labels = await page.evaluate(() => {
    const lum = (rgb: string) => {
      const m = rgb.match(/\d+(\.\d+)?/g)!.slice(0, 3).map(Number);
      const [r, g, b] = m.map((v) => {
        const s = v / 255;
        return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const scroller = document.querySelector("#ed0 .cm-scroller")!.getBoundingClientRect();
    return [...document.querySelectorAll("#ed0 .cm-mpc-caret-name")].map((el) => {
      const cs = getComputedStyle(el);
      const r = el.getBoundingClientRect();
      const [a, b] = [lum(cs.color), lum(cs.backgroundColor)];
      return {
        name: el.textContent,
        contrast: (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05),
        opacity: Number(cs.opacity),
        inside: r.top >= scroller.top - 0.5 && r.bottom <= scroller.bottom + 0.5 && r.height > 0,
      };
    });
  });
  expect(labels.length).toBe(colours.length);
  for (const l of labels) {
    expect(l.contrast, `${l.name}'s label contrast`).toBeGreaterThanOrEqual(4.5);
    expect(l.opacity, `${l.name}'s label shown`).toBe(1);
    expect(l.inside, `${l.name}'s label inside the editor`).toBe(true);
  }
});

test("a rejection's Re-apply and Dismiss work from the keyboard", async ({ page }) => {
  await load(page, "hello world\n");
  // editor 0 types "WORLD" and its socket drops with the edit (MPC-E000): Re-apply puts it back
  await page.evaluate(async () => {
    const sc = (window as any).sc;
    sc.editors[0].view.dispatch({ changes: { from: 6, insert: "WORLD" }, userEvent: "input.type" });
    sc.transports[0].drop(false);
    await sc.settle();
  });
  await settleLive(page); // the reconnect runs on real timers; its resync rejects the lost edit
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(1);
  await page.click("#ed0 .cm-content");
  let found = false;
  for (let i = 0; i < 12 && !found; i++) {
    await page.keyboard.press("Tab");
    found = await page.evaluate(() => document.activeElement?.classList.contains("mpc-reapply") ?? false);
  }
  expect(found, "Tab reaches Re-apply").toBe(true);
  await page.keyboard.press("Enter");
  await settle(page);
  let st = await state(page);
  expect(st.server).toContain("WORLD");
  expect(st.docs[0]).toBe(st.server);
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(0);

  await page.evaluate(async () => {
    const sc = (window as any).sc;
    sc.editors[0].view.dispatch({ changes: { from: 0, to: 5, insert: "HELLO" }, userEvent: "input.type" });
    sc.serverEdit([{ start: 0, end: 5, insert: "howdy" }]);
    await sc.settle();
  });
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(1);
  await page.focus("#ed0 .mpc-dismiss");
  await page.keyboard.press("Enter");
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(0);
  st = await state(page);
  expect(st.docs[0]).toBe(st.server);
});

test("a rejection Re-apply refuses (a conflict): from the keyboard, no text changes, the note is announced, Dismiss works", async ({ page }) => {
  await load(page, "hello world\n");
  // editor 0 replaces "world" while an outside edit replaces it first (MPC-E111): the server's
  // rule refuses it again, so Re-apply does
  await page.evaluate(async () => {
    const sc = (window as any).sc;
    sc.editors[0].view.dispatch({ changes: { from: 6, to: 11, insert: "WORLD" }, userEvent: "input.type" });
    sc.serverEdit([{ start: 6, end: 11, insert: "earth" }]);
    await sc.settle();
  });
  await settleLive(page);
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(1);
  const note = page.locator("#ed0 .mpc-note");
  await expect(note).toHaveAttribute("role", "status"); // a live region: its text is announced
  await expect(note).toBeHidden();
  const before = await state(page);
  expect(before.server).toBe("hello earth\n");
  await page.click("#ed0 .cm-content");
  let found = false;
  for (let i = 0; i < 12 && !found; i++) {
    await page.keyboard.press("Tab");
    found = await page.evaluate(() => document.activeElement?.classList.contains("mpc-reapply") ?? false);
  }
  expect(found, "Tab reaches Re-apply").toBe(true);
  await page.keyboard.press("Enter");
  await settle(page);
  // refused: the note says check and retype, nothing changed, the row is still there
  await expect(note).toBeVisible();
  await expect(note).toHaveText("Someone else changed this text since: nothing was re-applied. Check it, then type what's missing.");
  let st = await state(page);
  expect(st.server).toBe("hello earth\n");
  expect(st.docs).toEqual([st.server, st.server]);
  expect(st.serverVersion).toBe(before.serverVersion);
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(1);
  // Dismiss from the keyboard still works
  await page.focus("#ed0 .mpc-dismiss");
  await page.keyboard.press("Enter");
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(0);
  st = await state(page);
  expect(st.server).toBe("hello earth\n");
  expect(st.docs[0]).toBe(st.server);
  await noProblems(page);
});

/** Editor 0's "world" replaced by "WORLD" while an outside edit replaced it first: refused. */
async function conflictRow(page: Page) {
  await load(page, "hello world\n");
  await page.evaluate(async () => {
    const sc = (window as any).sc;
    sc.editors[0].view.dispatch({ changes: { from: 6, to: 11, insert: "WORLD" }, userEvent: "input.type" });
    sc.serverEdit([{ start: 6, end: 11, insert: "earth" }]);
    await sc.settle();
  });
  await settleLive(page);
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(1);
}

/** Tab from the editor until the focused element has class `cls`. */
async function tabTo(page: Page, cls: string) {
  await page.click("#ed0 .cm-content");
  for (let i = 0; i < 16; i++) {
    await page.keyboard.press("Tab");
    if (await page.evaluate((c) => document.activeElement?.classList.contains(c) ?? false, cls)) return;
  }
  throw new Error(`Tab never reached .${cls}`);
}

test("Insert my text here, from the keyboard: a preview where it goes, announced, then inserted", async ({ page }) => {
  await conflictRow(page);
  const note = page.locator("#ed0 .mpc-note");
  await expect(note).toHaveAttribute("role", "status");
  await tabTo(page, "mpc-insert-here");
  await page.keyboard.press("Enter");
  // the preview: shown in the editor, announced, nothing inserted yet, focus on Insert
  await expect(page.locator("#ed0 .cm-mpc-insert-preview")).toHaveText("WORLD");
  await expect(page.locator("#ed0 .cm-mpc-insert-preview")).toBeVisible();
  await expect(note).toBeVisible();
  await expect(note).toHaveText("Your text will go in at line 1, shown in the editor. Insert it, or Cancel. Your edit also removed text; that part isn't redone here.");
  expect((await state(page)).server).toBe("hello earth\n");
  expect(await page.evaluate(() => document.activeElement?.classList.contains("mpc-insert-confirm") ?? false)).toBe(true);
  await page.keyboard.press("Enter");
  await settle(page);
  const st = await state(page);
  expect(st.server).toBe("hello WORLDearth\n");
  expect(st.docs).toEqual([st.server, st.server]);
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(0);
  await expect(page.locator("#ed0 .cm-mpc-insert-preview")).toHaveCount(0);
});

test("Insert my text here: Escape cancels, nothing inserted, focus back on the action", async ({ page }) => {
  await conflictRow(page);
  await tabTo(page, "mpc-insert-here");
  await page.keyboard.press("Enter");
  await expect(page.locator("#ed0 .cm-mpc-insert-preview")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator("#ed0 .cm-mpc-insert-preview")).toHaveCount(0);
  await expect(page.locator("#ed0 .mpc-note")).toBeHidden();
  expect(await page.evaluate(() => document.activeElement?.classList.contains("mpc-insert-here") ?? false)).toBe(true);
  await settle(page);
  expect((await state(page)).server).toBe("hello earth\n");
  await expect(page.locator("#ed0 .mpc-rejections li")).toHaveCount(1);
});

test("the rejected text is always copyable, from the keyboard (the clipboard, or selected when it refuses)", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]).catch(() => {});
  await conflictRow(page);
  await tabTo(page, "mpc-copy");
  await page.keyboard.press("Enter");
  const note = page.locator("#ed0 .mpc-note");
  await expect(note).toBeVisible();
  const said = await note.textContent();
  if (said === "Copied.") {
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("WORLD");
  } else {
    expect(said).toBe("Couldn't copy: the text is selected, copy it with your keyboard.");
    expect(await page.evaluate(() => document.getSelection()?.toString())).toBe("WORLD");
  }
});

test("a dropped socket: the banner, then the resume, and typing goes on", async ({ page }) => {
  await load(page, "abc\n");
  await page.evaluate(async () => {
    const sc = (window as any).sc;
    sc.transports[0].failConnects = 2;
    sc.transports[0].drop();
    await sc.settle();
  });
  await expect(page.locator("#ed0 .mpc-banner")).toBeVisible();
  await page.click("#ed0 .cm-content");
  await page.keyboard.press("Control+End");
  await page.keyboard.type("typed offline");
  await page.evaluate(() => (window as any).sc.type(1, 0, "B"));
  await settleLive(page);
  await expect(page.locator("#ed0 .mpc-banner")).toBeHidden();
  const st = await state(page);
  expect(st.server).toContain("typed offline");
  expect(st.server.startsWith("B")).toBe(true);
  expect(st.docs[0]).toBe(st.server);
  expect(st.docs[1]).toBe(st.server);
  expect(st.states).toEqual(["live", "live"]);
  await noProblems(page);
});

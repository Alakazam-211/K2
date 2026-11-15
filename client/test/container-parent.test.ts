// Containers in containers (WIRE.md, "Containers in containers"): a container's `parent`, by
// address, from the snapshot and each change's `parent_after`. The kit's own frames
// (`test/golden/keyed_frames_container_parent.json`, written by
// `crates/base/tests/view_containers.rs`) replay to the kit's tree after the snapshot and after
// every change: a parent read from a field (the store holds the parent's key, the view its
// address: the client never maps one to the other), and one the pack derives from another root
// (a `props` delta naming no field). A snapshot from a pack whose containers don't nest has no
// `parent` at all.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KEYED_WATCH_FEATURE, KeyedWatch } from "../src/keyed";
import { CONTAINER_DEPTH_MAX } from "../src/index";
import type { ClientFrame, ServerFrame, Transport, Welcome } from "../src/wire";

const HERE = dirname(fileURLToPath(import.meta.url));
type Run = { frames: ServerFrame[]; final_json: string; final_rev: number; parents: Record<string, string | null>[] };
type Golden = { features: string[]; field: Run; derived: Run };
const GOLDEN = JSON.parse(readFileSync(join(HERE, "golden", "keyed_frames_container_parent.json"), "utf8")) as Golden;

function wire() {
  const ls = new Set<(f: ServerFrame) => void>();
  const sent: ClientFrame[] = [];
  const t: Transport = {
    send: (f) => void sent.push(f),
    onFrame: (cb) => {
      ls.add(cb);
      return () => ls.delete(cb);
    },
  };
  return { t, sent, deliver: (f: ServerFrame) => [...ls].forEach((l) => l(f)) };
}

const welcome: Welcome = { root: "objs", version: 1, client: 7, self: "ada", rights: "read", features: [KEYED_WATCH_FEATURE] };

/** Each container's parent as the client's view holds it (`null`: at the top level). */
const parentsOf = (k: KeyedWatch) => Object.fromEntries((k.view?.containers ?? []).map((c) => [c.key, c.parent ?? null]));

/** The run's frames delivered one by one: the tree after the snapshot (every frame before the
 * first change) and after each change. */
function replay(run: Run) {
  const w = wire();
  const k = new KeyedWatch(w.t, welcome);
  const trees: Record<string, string | null>[] = [];
  let snapshotDone = false;
  for (const f of run.frames) {
    if (f.type === "ops_change" && !snapshotDone) {
      trees.push(parentsOf(k));
      snapshotDone = true;
    }
    w.deliver(f);
    if (f.type === "ops_change") trees.push(parentsOf(k));
  }
  return { k, trees, sent: w.sent };
}

describe("containers in containers", () => {
  for (const mode of ["field", "derived"] as const) {
    it(`${mode}: the kit's frames replay to the kit's tree after the snapshot and after every change`, () => {
      const run = GOLDEN[mode];
      const { k, trees, sent } = replay(run);
      expect(k.ready).toBe(true);
      expect(k.viewCurrent).toBe(true);
      expect(k.stateJson()).toBe(run.final_json);
      expect(k.rev).toBe(run.final_rev);
      expect(trees).toEqual(run.parents);
      expect(sent.filter((f) => f.type === "watch").length).toBe(1);
    });
  }

  it("field: the store holds the parent's key, the view its address", () => {
    const run = GOLDEN.field;
    const { k } = replay(run);
    // the first change set @coll:3's parent field to the key `c4`
    expect(run.final_json).toContain("c4");
    expect(k.view?.containers.find((c) => c.key === "@coll:3")?.parent).toBe("@coll:4");
  });

  it("derived: a parent that changed with no op on its container comes as a props delta naming no field", () => {
    const changes = GOLDEN.derived.frames.filter((f) => f.type === "ops_change") as unknown as { containers_changed: { key: string; op: string; fields: string[]; parent_after?: string | null }[] }[];
    const first = changes[0].containers_changed.find((d) => d.key === "@coll:3");
    expect(first).toMatchObject({ op: "props", fields: [], parent_after: "@coll:4" });
  });

  it("a top-level container has no parent on the wire; the depth limit is the kit's", () => {
    const head = GOLDEN.field.frames[0] as unknown as { view: { containers: Record<string, unknown>[] } };
    const top = head.view.containers.find((c) => c.key === "@coll:1");
    expect(top && "parent" in top).toBe(false);
    expect(CONTAINER_DEPTH_MAX).toBe(32);
  });

  it("a snapshot from a pack whose containers don't nest has no parent at all", () => {
    const g = JSON.parse(readFileSync(join(HERE, "golden", "keyed_snapshot.json"), "utf8")) as { view: { frames: ServerFrame[] } };
    const text = JSON.stringify(g.view.frames);
    expect(text.includes('"parent_after"')).toBe(false);
    const head = g.view.frames[0] as unknown as { view: { containers: Record<string, unknown>[]; schema: { container_fields: Record<string, unknown> } } };
    expect(head.view.containers.some((c) => "parent" in c)).toBe(false);
    expect("parent" in head.view.schema.container_fields).toBe(false);
  });
});

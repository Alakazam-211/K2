// Writer leases (kit v0.14.0, person-writer reuse): a host that reuses a person's writer per
// device sends `lease` in `welcome`, gives a reconnecting client its writer back when its lease
// is still its (`kept`), and otherwise says when that lease ended (`prevEnded`). The client must
// then never read another connection's changes under its old writer as its own lost change
// (R11, the false landing), settles its edit in flight for good on a kept reconnect, and says
// `bye` only when it holds nothing unsent.

import { describe, expect, it } from "vitest";
import type { Hunk } from "../src/hunk";
import { LiveText, type Landed, type Rejection } from "../src/live";
import type { ChangeEvent, ClientFrame, ServerFrame, Transport } from "../src/wire";

/** A transport the test drives by hand. */
function wire() {
  const sent: ClientFrame[] = [];
  const ls = new Set<(f: ServerFrame) => void>();
  const t: Transport = {
    send: (f) => sent.push(f),
    onFrame: (cb) => {
      ls.add(cb);
      return () => ls.delete(cb);
    },
  };
  const deliver = (f: ServerFrame) => ls.forEach((l) => l(f));
  return { t, sent, deliver };
}

const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });
const ev = (version: number, client: number, hunks: Hunk[]): ChangeEvent => ({
  version,
  pid: "p1",
  client,
  via: "tab",
  kind: "edit",
  root: "source",
  hunks,
  at_ms: 0,
});
const lastSince = (sent: ClientFrame[]) => {
  const s = sent.filter((f): f is Extract<ClientFrame, { type: "since" }> => f.type === "since");
  return s[s.length - 1];
};

/** A tab on writer 7 (lease "11") with an edit in flight when its socket goes. */
function lostEdit() {
  const w = wire();
  const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 7, lease: "11" });
  const rejected: Rejection[] = [];
  const landed: Landed[] = [];
  l.onRejected((r) => rejected.push(r));
  l.onLanded((x) => landed.push(x));
  l.applyLocal([h(2, 2, "s")], { group: "g1" }); // in flight
  l.disconnected();
  return { w, l, rejected, landed };
}

describe("writer leases (kit v0.14.0)", () => {
  it("a kept reconnect settles the edit in flight for good: no lost-change watch (lease_kept_settles)", () => {
    const { w, l, rejected, landed } = lostEdit();
    l.reconnected({ client: 7, lease: "12", kept: true });
    expect(l.lease).toBe("12");
    w.deliver({ type: "events", op: lastSince(w.sent).op, events: [] }); // not listed: not applied
    expect(rejected).toHaveLength(1);
    expect(rejected[0].asOf).toBeUndefined();
    // the kept writer's own next change, with the same hunks: never read as the lost one
    w.deliver({ type: "change", event: ev(2, 7, [h(2, 2, "s")]) });
    expect(landed).toHaveLength(0);
  });

  it("not kept: another connection's change under the old writer after its lease ended is not the lost change (lease_cap_r11)", () => {
    const { w, l, rejected, landed } = lostEdit();
    l.reconnected({ client: 9, lease: "21", kept: false, prevEnded: 1 });
    w.deliver({ type: "events", op: lastSince(w.sent).op, events: [] });
    expect(rejected).toHaveLength(1);
    expect(rejected[0].asOf).toMatchObject({ client: 7, until: 1 });
    // a sibling given writer 7 later types the same thing
    w.deliver({ type: "change", event: ev(2, 7, [h(2, 2, "s")]) });
    expect(landed).toHaveLength(0);
    expect(l.presence(rejected[0].id)).toBe("absent");
  });

  it("not kept, the end unknown: the watch concludes nothing (lease_cap_unknown)", () => {
    const { w, l, rejected, landed } = lostEdit();
    l.reconnected({ client: 9, lease: "21", kept: false });
    w.deliver({ type: "events", op: lastSince(w.sent).op, events: [] });
    w.deliver({ type: "change", event: ev(2, 7, [h(2, 2, "s")]) });
    expect(landed).toHaveLength(0);
    expect(l.presence(rejected[0].id)).toBe("unknown");
  });

  it("not kept: the edit landing before its lease ended is still recognised (lease_cap_landed)", () => {
    const { w, l, rejected } = lostEdit();
    l.reconnected({ client: 9, lease: "21", kept: false, prevEnded: 2 });
    w.deliver({ type: "events", op: lastSince(w.sent).op, events: [ev(2, 7, [h(2, 2, "s")])] });
    expect(rejected).toHaveLength(0);
    expect(l.text).toBe("abs");
    expect(l.pending).toBe(0);
  });

  it("a host without leases: as before, the old writer's later change is read as the landing (lease_old_host)", () => {
    const { w, l, rejected, landed } = lostEdit();
    l.reconnected({ client: 9 });
    w.deliver({ type: "events", op: lastSince(w.sent).op, events: [] });
    expect(rejected[0].asOf).toMatchObject({ client: 7 });
    expect(rejected[0].asOf?.until).toBeUndefined();
    w.deliver({ type: "change", event: ev(2, 7, [h(2, 2, "s")]) });
    expect(landed).toHaveLength(1);
  });

  it("bye only with nothing unsent; it drops the lease (lease_bye)", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 7, lease: "11" });
    l.applyLocal([h(2, 2, "s")], { group: "g1" });
    expect(l.bye()).toBe(false); // an edit in flight
    const edit = w.sent.find((f): f is Extract<ClientFrame, { type: "edit" }> => f.type === "edit")!;
    w.deliver({ type: "ack", op: edit.op, version: 2, hunks: [h(2, 2, "s")] });
    expect(l.pending).toBe(0);
    expect(l.bye()).toBe(true);
    expect(w.sent[w.sent.length - 1]).toEqual({ type: "bye" });
    expect(l.lease).toBeUndefined();
    expect(l.bye()).toBe(false); // once
    const old = new LiveText({ text: "ab", version: 1 }, wire().t, { client: 8 });
    expect(old.bye()).toBe(false); // a host without leases: nothing to release
  });
});

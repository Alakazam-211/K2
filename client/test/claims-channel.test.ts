// `PresenceChannel.claim` and `release`: they send only to a host that lists `"claims"`, carry
// `req` and never `op`, resolve on `claimed` and `released`, and reject on `presence_refused`,
// `close()` and a reconnect, saying that a request caught by the last two has an unknown fate.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LiveText, type Rejection } from "../src/live";
import { CLAIM_KEYS_MAX, CLAIM_NOTE_MAX, ClaimRejected, PresenceChannel, type PresenceRefusal } from "../src/presence";
import type { ClientFrame, ErrorReport, ServerFrame, Transport, Welcome } from "../src/wire";
import { withClaimsSentAnyway } from "./plants";

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

const welcome = (over: Partial<Welcome> = {}): Welcome => ({
  root: "source",
  version: 1,
  client: 7,
  self: "me",
  rights: "edit",
  features: ["claims"],
  ...over,
});

const now = { schedule: (deliver: () => void) => deliver() };

const refusal = (id: string, message = "m"): ErrorReport => ({ id, message });

/** The channel's frames of one request, by its number. */
const reqOf = (f: ClientFrame | undefined) => (f as { req: number }).req;

/** `MPC_CLIENT_PLANT` runs every test here under the planted fault, so a run shows which ones
 * catch it: `claims-sent-anyway` is `withClaimsSentAnyway`. Any other value throws. */
const plant = process.env.MPC_CLIENT_PLANT;
if (plant !== undefined && plant !== "claims-sent-anyway") throw new Error(`MPC_CLIENT_PLANT=${plant}: not a plant this suite has`);
let plantSpy: { mockRestore(): void } | null = null;
beforeEach(() => {
  if (plant) plantSpy = vi.spyOn(PresenceChannel.prototype, "claimsSupported", "get").mockReturnValue(true);
});
afterEach(() => {
  plantSpy?.mockRestore();
  plantSpy = null;
});

/** Everything a channel would send, and how each request ended, on a connection with `w`. */
function askAll(w: Welcome) {
  const { t, sent } = wire();
  const c = new PresenceChannel(t, w, now);
  const asked = [c.claim("items", ["i1"], { hold: true }), c.release("claim-1"), c.release()];
  // nothing here answers: a plant's pending requests just stay pending
  for (const p of asked) p.catch(() => {});
  return { sent, asked };
}

describe("a host without the feature", () => {
  const { features: _, ...noField } = welcome();
  const cases: [string, Welcome][] = [
    ["no `features` field", noField],
    ["an empty list", welcome({ features: [] })],
    ["only other names", welcome({ features: ["presence", "claims2", "CLAIMS"] })],
  ];
  for (const [why, w] of cases) {
    it(`${why}: it never receives a claim or release frame`, async () => {
      const { sent, asked } = askAll(w);
      expect(sent).toEqual([]);
      for (const p of asked) await expect(p).rejects.toMatchObject({ name: "ClaimRejected", reason: "unsupported", error: null });
    });
    it(`${why}: the plant that sends anyway is caught`, () => {
      expect(withClaimsSentAnyway(() => askAll(w)).sent.length).toBe(3);
    });
  }

  it("the two families are separate: `presence` alone sends no claim, `claims` alone no selection", () => {
    const a = wire();
    const p = new PresenceChannel(a.t, welcome({ features: ["presence"] }), now);
    p.claim("items", ["i1"]).catch(() => {});
    p.setSelection({ root: "items", keys: ["i1"] });
    if (!plant) expect(a.sent.map((f) => f.type)).toEqual(["select"]);
    const b = wire();
    const c = new PresenceChannel(b.t, welcome({ features: ["claims"] }), now);
    c.setSelection({ root: "items", keys: ["i1"] });
    c.claim("items", ["i1"]).catch(() => {});
    expect(b.sent.map((f) => f.type)).toEqual(["claim"]);
  });

  it("the check is per connection: a reconnect's welcome replaces the list", async () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    expect(c.claimsSupported).toBe(true);
    c.reconnected(welcome({ features: [] }));
    expect(c.claimsSupported).toBe(plant !== undefined);
    if (!plant) await expect(c.claim("items", ["i1"])).rejects.toMatchObject({ reason: "unsupported" });
    if (!plant) expect(sent).toEqual([]);
    c.reconnected(welcome());
    c.claim("items", ["i1"]).catch(() => {});
    expect(sent.at(-1)?.type).toBe("claim");
  });
});

describe("the frames", () => {
  it("carry `req`, never `op` or `seq`, only the fields asked for, from the channel's one counter", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome({ features: ["presence", "claims"] }), now);
    c.setSelection({ root: "items", keys: ["i1"] });
    c.claim("items", ["i2", "i1", "i2"]).catch(() => {});
    c.claim("items", ["i3"], { ttlMs: 5000, note: "moving", hold: true }).catch(() => {});
    c.release().catch(() => {});
    c.release("claim-9").catch(() => {});
    expect(sent).toEqual([
      { type: "select", req: 1, selection: { root: "items", keys: ["i1"] } },
      { type: "claim", req: 2, root: "items", keys: ["i2", "i1"] },
      { type: "claim", req: 3, root: "items", keys: ["i3"], ttl_ms: 5000, note: "moving", hold: true },
      { type: "release", req: 4 },
      { type: "release", req: 5, claim: "claim-9" },
    ]);
  });
});

describe("the answers", () => {
  it("`claimed` and `released` resolve the request they name, in any order", async () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const a = c.claim("items", ["i1"]);
    const b = c.claim("items", ["i2"]);
    const r = c.release("claim-1");
    deliver({ type: "released", req: reqOf(sent[2]), claims: ["claim-1"] });
    deliver({ type: "claimed", req: reqOf(sent[1]), claim: "claim-2" });
    deliver({ type: "claimed", req: reqOf(sent[0]), claim: "claim-1" });
    await expect(a).resolves.toBe("claim-1");
    await expect(b).resolves.toBe("claim-2");
    await expect(r).resolves.toEqual(["claim-1"]);
  });

  it("a release of nothing live resolves to no ids", async () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const r = c.release();
    deliver({ type: "released", req: reqOf(sent[0]), claims: [] });
    await expect(r).resolves.toEqual([]);
  });

  it("a reply of the wrong type or an unknown number answers nothing", async () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const a = c.claim("items", ["i1"]);
    let settled = false;
    a.then(() => (settled = true), () => (settled = true));
    deliver({ type: "released", req: reqOf(sent[0]), claims: [] });
    deliver({ type: "claimed", req: 999, claim: "claim-9" });
    await Promise.resolve();
    expect(settled).toBe(false);
    deliver({ type: "claimed", req: reqOf(sent[0]), claim: "claim-1" });
    await expect(a).resolves.toBe("claim-1");
  });
});

describe("rejection", () => {
  it("`presence_refused`: rejects with the host's error, and `onRefused` hears it", async () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const heard: PresenceRefusal[] = [];
    c.onRefused((r) => heard.push(r));
    const a = c.claim("items", ["i1"]);
    const r = c.release("claim-1");
    const taken = refusal("MPC-E301", "items i1 claimed by Bo (bo); 900 ms left");
    const none = refusal("MPC-E302", "no live claim claim-1");
    deliver({ type: "presence_refused", req: reqOf(sent[0]), error: taken });
    deliver({ type: "presence_refused", req: reqOf(sent[1]), error: none });
    const why = await a.catch((e: unknown) => e);
    expect(why).toBeInstanceOf(ClaimRejected);
    expect(why).toMatchObject({ reason: "refused", error: taken, message: taken.message });
    await expect(r).rejects.toMatchObject({ reason: "refused", error: none });
    expect(heard).toEqual([
      { frame: "claim", error: taken },
      { frame: "release", error: none },
    ]);
    // answered: a repeat finds nothing to reject, and only says so without a frame
    deliver({ type: "presence_refused", req: reqOf(sent[0]), error: taken });
    expect(heard.at(-1)).toEqual({ frame: null, error: taken });
  });

  it("`close()`: rejects what is waiting, says its fate is unknown, and nothing goes out after", async () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const a = c.claim("items", ["i1"]);
    const r = c.release();
    c.close();
    for (const p of [a, r]) {
      const why = (await p.catch((e: unknown) => e)) as ClaimRejected;
      expect(why).toBeInstanceOf(ClaimRejected);
      expect(why.reason).toBe("closed");
      expect(why.error).toBeNull();
      expect(why.message).toContain("fate is unknown");
    }
    const n = sent.length;
    await expect(c.claim("items", ["i1"])).rejects.toMatchObject({ reason: "closed" });
    await expect(c.release()).rejects.toMatchObject({ reason: "closed" });
    expect(sent.length).toBe(n);
    // no frame is read: a late answer does nothing
    deliver({ type: "claimed", req: reqOf(sent[0]), claim: "claim-1" });
  });

  it("a reconnect: rejects what is waiting, says plainly that a claim ends with its connection, and sends no claim again", async () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const a = c.claim("items", ["i1"], { hold: true });
    const r = c.release("claim-1");
    const before = sent.length;
    c.reconnected(welcome({ client: 8 }));
    for (const p of [a, r]) {
      const why = (await p.catch((e: unknown) => e)) as ClaimRejected;
      expect(why).toBeInstanceOf(ClaimRejected);
      expect(why.reason).toBe("reconnected");
      expect(why.error).toBeNull();
      expect(why.message).toContain("fate is unknown");
      expect(why.message).toContain("has ended, or will end when the old connection closes");
      expect(why.message).toContain("doesn't outlive its connection, held or not");
      expect(why.message).toContain("claim again");
    }
    expect(sent.length).toBe(before);
    // the old connection's late answer is nobody's, and the new connection's numbers don't reuse it
    deliver({ type: "claimed", req: reqOf(sent[0]), claim: "claim-1" });
    const next = c.claim("items", ["i1"]);
    expect(reqOf(sent.at(-1))).toBeGreaterThan(reqOf(sent[before - 1]));
    deliver({ type: "claimed", req: reqOf(sent.at(-1)), claim: "claim-2" });
    await expect(next).resolves.toBe("claim-2");
  });

  it("what the host would refuse is refused here, unsent, by `onRefused` too", async () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const heard: PresenceRefusal[] = [];
    c.onRefused((r) => heard.push(r));
    const many = Array.from({ length: CLAIM_KEYS_MAX + 1 }, (_, i) => `k${i}`);
    const bad = [
      c.claim("items", []),
      c.claim("items", many),
      c.claim("items", ["i1"], { note: "é".repeat(CLAIM_NOTE_MAX / 2 + 1) }),
      c.claim("items", ["i1"], { ttlMs: 0 }),
      c.claim("items", ["i1"], { ttlMs: 1.5 }),
      c.claim("items", ["i1"], { ttlMs: Number.NaN }),
    ];
    for (const p of bad) {
      await expect(p).rejects.toMatchObject({ reason: "refused", error: { id: "MPC-E304" } });
    }
    expect(sent).toEqual([]);
    expect(heard.map((h) => [h.frame, h.error.id])).toEqual(bad.map(() => ["claim", "MPC-E304"]));
  });

  it("exactly the most goes out", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const most = Array.from({ length: CLAIM_KEYS_MAX }, (_, i) => `k${i}`);
    c.claim("items", most, { note: "n".repeat(CLAIM_NOTE_MAX) }).catch(() => {});
    expect(sent.length).toBe(1);
  });
});

describe("on a transport shared with LiveText", () => {
  it("neither reads the other's replies, even with the same number", async () => {
    const w = wire();
    const rejected: Rejection[] = [];
    const l = new LiveText({ text: "abc", version: 1 }, w.t);
    l.onRejected((r) => rejected.push(r));
    const c = new PresenceChannel(w.t, welcome(), now);
    l.applyLocal([{ start: 0, end: 0, insert: "x" }]);
    const edit = w.sent.find((f) => f.type === "edit") as { op: number };
    // a first request, so the second's number isn't the edit's
    c.claim("items", ["i0"]).catch(() => {});
    const a = c.claim("items", ["i1"]);
    const req = reqOf(w.sent.at(-1));
    expect(req).not.toBe(edit.op);
    // an edit's refusal with the claim's number leaves the claim waiting
    let settled = false;
    a.then(() => (settled = true), () => (settled = true));
    w.deliver({ type: "refused", op: req, error: refusal("MPC-E111") });
    await Promise.resolve();
    expect(settled).toBe(false);
    // a claim's refusal with the edit's number leaves the edit in flight
    w.deliver({ type: "presence_refused", req: edit.op, error: refusal("MPC-E301") });
    expect(rejected).toEqual([]);
    expect(l.text).toBe("xabc");
    w.deliver({ type: "ack", op: edit.op, version: 2, hunks: [{ start: 0, end: 0, insert: "x" }] });
    expect(l.version).toBe(2);
    a.catch(() => {});
  });
});

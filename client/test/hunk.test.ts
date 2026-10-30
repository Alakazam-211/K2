import { describe, expect, it } from "vitest";
import { apply, byteToIndex, coalesce, compose, indexToByte, invert, utf8Len, validate, type Hunk } from "../src/hunk";
import { diffText } from "../src/live";
import { rng } from "./fake-server";

const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });

describe("UTF-8 offsets", () => {
  it("counts bytes and converts at character boundaries", () => {
    const t = "aé日🎉b"; // 1 + 2 + 3 + 4 + 1
    expect(utf8Len(t)).toBe(11);
    expect([0, 1, 3, 6, 10, 11].map((b) => byteToIndex(t, b))).toEqual([0, 1, 2, 3, 5, 6]);
    expect(byteToIndex(t, 2)).toBe(-1); // inside é
    expect(byteToIndex(t, 8)).toBe(-1); // inside 🎉
    expect(byteToIndex(t, 12)).toBe(-1);
    expect([0, 1, 2, 3, 5, 6].map((i) => indexToByte(t, i))).toEqual([0, 1, 3, 6, 10, 11]);
  });
});

describe("apply / validate / coalesce", () => {
  it("apply works in bytes", () => {
    expect(apply("aé日b", [h(1, 3, "E"), h(6, 6, "!")])).toBe("aE日!b");
  });
  it("validate refuses what the kit refuses", () => {
    expect(validate("abc", [h(1, 2), h(2, 3)])).toBeNull();
    expect(validate("abc", [h(2, 3), h(1, 2)])?.kind).toBe("order");
    expect(validate("abc", [h(1, 1, "x"), h(1, 1, "y")])?.kind).toBe("order");
    expect(validate("abc", [h(1, 1)])?.kind).toBe("empty");
    expect(validate("é", [h(1, 1, "x")])?.kind).toBe("range");
    expect(validate("abc", [h(0, 4)])?.kind).toBe("range");
    expect(validate("abc", [h(1, 1, "x"), h(1, 2)])).toBeNull(); // insert then delete at a point
  });
  it("coalesce joins touching hunks", () => {
    expect(coalesce([h(1, 2), h(2, 2, "x"), h(4, 5, "y")])).toEqual([h(1, 2, "x"), h(4, 5, "y")]);
  });
});

function boundaries(t: string) {
  const out = [0];
  let b = 0;
  for (const ch of t) out.push((b += utf8Len(ch)));
  return out;
}

function randomHunks(r: ReturnType<typeof rng>, t: string): Hunk[] {
  const b = boundaries(t);
  const cuts = Array.from({ length: 2 + 2 * r.int(3) }, () => r.pick(b)).sort((x, y) => x - y);
  const out: Hunk[] = [];
  for (let i = 0; i < cuts.length; i += 2) {
    const s = cuts[i];
    const e = r.next() < 0.4 ? s : cuts[i + 1];
    const ins = ["", "x", "é", "日日", "🎉"][r.int(5)];
    const last = out[out.length - 1];
    if (last && (s < last.end || (s === last.end && s === e))) continue;
    if (s === e && !ins) continue;
    out.push(h(s, e, ins));
  }
  return coalesce(out);
}

describe("compose and invert", () => {
  it("compose(a, b) applies as a then b; invert undoes", () => {
    const r = rng(42);
    const alphabet = ["a", "b", "é", "日", "🎉", "\n"];
    for (let i = 0; i < 3000; i++) {
      let t0 = "";
      for (let n = r.int(9); n > 0; n--) t0 += r.pick(alphabet);
      const a = randomHunks(r, t0);
      const t1 = apply(t0, a);
      const b = randomHunks(r, t1);
      const t2 = apply(t1, b);
      const c = compose(t0, a, b);
      expect(validate(t0, c), JSON.stringify({ t0, a, b, c })).toBeNull();
      expect(apply(t0, c), JSON.stringify({ t0, a, b, c })).toBe(t2);
      expect(apply(t1, invert(t0, a))).toBe(t0);
      const d = diffText(t0, t2);
      expect(apply(t0, d)).toBe(t2);
    }
  });
});

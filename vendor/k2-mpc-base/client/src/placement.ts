// A unit's placement in 3D as a matrix (WIRE.md, "Placements in 3D"; the kit's
// `docs/design/view-3d-slot-r1.1.md`, section 1.2): the same formulas as the kit's Rust helper,
// tested on one shared set of vectors (`test/vectors/placement.json`). Data, no DOM.
//
// - Matrices are `Float64Array(16)`, column-major (`m[col * 4 + row]`, the translation in
//   `m[12..14]`), for column vectors, right-handed, radians.
// - A placement a reader can't read whole (a step, rotation or order word it doesn't know) is
//   unknown: `null`, never one with a step skipped. There is no step limit here.
// - A degenerate form (a zero quaternion or axis, a reference direction along Z, a matrix whose
//   last row isn't exactly 0, 0, 0, 1) gives `null`.
// - `Math.sin` differs between engines: results match the vectors to a tolerance, not the bit.

import type { PlacementJson, RotationJson, StepJson } from "./wire";

export type Mat4 = Float64Array;
export type Precision = "f32" | "f64";

const ORDERS = ["XYZ", "XZY", "YXZ", "YZX", "ZXY", "ZYX"];

function round(x: unknown, p: Precision): number | null {
  if (typeof x !== "number" || !Number.isFinite(x)) return null;
  return p === "f32" ? Math.fround(x) : x;
}

function vec(v: unknown, n: number, p: Precision): number[] | null {
  if (!Array.isArray(v) || v.length !== n) return null;
  const out: number[] = [];
  for (const x of v) {
    const r = round(x, p);
    if (r === null) return null;
    out.push(r);
  }
  return out;
}

function identity(): Mat4 {
  const m = new Float64Array(16);
  m[0] = m[5] = m[10] = m[15] = 1;
  return m;
}

/** `a · b`, column-major. */
export function mul(a: Mat4, b: Mat4): Mat4 {
  const out = new Float64Array(16);
  for (let c = 0; c < 4; c++)
    for (let r = 0; r < 4; r++) {
      let s = 0;
      for (let k = 0; k < 4; k++) s += a[k * 4 + r] * b[c * 4 + k];
      out[c * 4 + r] = s;
    }
  return out;
}

type M3 = number[][];

function m3mul(a: M3, b: M3): M3 {
  return [0, 1, 2].map((r) => [0, 1, 2].map((c) => a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c]));
}

function basic(axis: number, a: number): M3 {
  const s = Math.sin(a);
  const c = Math.cos(a);
  if (axis === 0) return [[1, 0, 0], [0, c, -s], [0, s, c]];
  if (axis === 1) return [[c, 0, s], [0, 1, 0], [-s, 0, c]];
  return [[c, -s, 0], [s, c, 0], [0, 0, 1]];
}

function norm(v: number[]): number {
  return Math.sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
}

function unit(v: number[]): number[] | null {
  const n = norm(v);
  return n > 0 && Number.isFinite(n) ? [v[0] / n, v[1] / n, v[2] / n] : null;
}

function rotation(r: RotationJson | unknown, p: Precision): M3 | null {
  if (!r || typeof r !== "object") return null;
  const o = r as Record<string, unknown>;
  const keys = Object.keys(o).sort().join(",");
  if (keys === "angles,euler") {
    const order = ORDERS.indexOf(String(o.euler));
    const a = vec(o.angles, 3, p);
    if (order < 0 || !a) return null;
    const axes = [...(o.euler as string)].map((ch) => "XYZ".indexOf(ch));
    const ra = basic(axes[0], a[axes[0]]);
    const rb = basic(axes[1], a[axes[1]]);
    const rc = basic(axes[2], a[axes[2]]);
    return m3mul(m3mul(rc, rb), ra);
  }
  if (keys === "quat") {
    const q = vec(o.quat, 4, p);
    if (!q) return null;
    const n = Math.sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]);
    if (!(n > 0) || !Number.isFinite(n)) return null;
    const [w, x, y, z] = q.map((v) => v / n);
    return [
      [1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y)],
      [2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x)],
      [2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)],
    ];
  }
  if (keys === "angle,axis") {
    const ax = vec(o.axis, 3, p);
    const t = round(o.angle, p);
    if (!ax || t === null) return null;
    const u = unit(ax);
    if (!u) return null;
    const [x, y, z] = u;
    const s = Math.sin(t);
    const c = Math.cos(t);
    const k = 1 - c;
    return [
      [c + x * x * k, x * y * k - z * s, x * z * k + y * s],
      [y * x * k + z * s, c + y * y * k, y * z * k - x * s],
      [z * x * k - y * s, z * y * k + x * s, c + z * z * k],
    ];
  }
  return null;
}

function affine(m: M3, t: number[]): Mat4 {
  const out = identity();
  for (let r = 0; r < 3; r++) for (let c = 0; c < 3; c++) out[c * 4 + r] = m[r][c];
  out[12] = t[0];
  out[13] = t[1];
  out[14] = t[2];
  return out;
}

/** One step's matrix, or `null` for a degenerate or unknown step. */
export function stepMatrix(step: StepJson | unknown, p: Precision = "f64"): Mat4 | null {
  if (!step || typeof step !== "object") return null;
  const o = step as Record<string, unknown>;
  const kind = Object.keys(o);
  if (kind.length !== 1) return null;
  const body = o[kind[0]] as Record<string, unknown> | null;
  if (!body || typeof body !== "object") return null;
  switch (kind[0]) {
    case "trs": {
      const loc = vec(body.loc, 3, p);
      const scale = vec(body.scale, 3, p);
      const r = rotation(body.rot, p);
      if (!loc || !scale || !r) return null;
      return affine(r.map((row) => row.map((v, c) => v * scale[c])), loc);
    }
    case "axes": {
      const origin = vec(body.origin, 3, p);
      if (!origin) return null;
      let z = [0, 0, 1];
      if (body.z !== null && body.z !== undefined) {
        const d = vec(body.z, 3, p);
        const u = d && unit(d);
        if (!u) return null;
        z = u;
      }
      let v: number[];
      if (body.x !== null && body.x !== undefined) {
        const d = vec(body.x, 3, p);
        if (!d || !(norm(d) > 0)) return null;
        v = d;
      } else v = z[0] === 1 && z[1] === 0 && z[2] === 0 ? [0, 1, 0] : [1, 0, 0];
      const dot = v[0] * z[0] + v[1] * z[1] + v[2] * z[2];
      const proj = [v[0] - dot * z[0], v[1] - dot * z[1], v[2] - dot * z[2]];
      const len = norm(proj);
      if (!(len > 1e-12 * norm(v))) return null;
      const x = proj.map((c) => c / len);
      const y = [z[1] * x[2] - z[2] * x[1], z[2] * x[0] - z[0] * x[2], z[0] * x[1] - z[1] * x[0]];
      return affine(
        [
          [x[0], y[0], z[0]],
          [x[1], y[1], z[1]],
          [x[2], y[2], z[2]],
        ],
        origin,
      );
    }
    case "matrix": {
      const m = vec(body, 16, p);
      if (!m) return null;
      if (m[3] !== 0 || m[7] !== 0 || m[11] !== 0 || m[15] !== 1) return null;
      return Float64Array.from(m);
    }
    default:
      return null;
  }
}

/** A placement's matrix: the product of its steps (the first outermost), or `null` when it is
 * unknown or a step is degenerate. Any number of steps. */
export function placementMatrix(p: PlacementJson | null | undefined, precision: Precision = "f64"): Mat4 | null {
  if (!p || !Array.isArray(p.steps) || p.steps.length === 0) return null;
  let m = identity();
  for (const s of p.steps) {
    const sm = stepMatrix(s, precision);
    if (!sm) return null;
    m = mul(m, sm);
  }
  return m.every((v) => Number.isFinite(v)) ? m : null;
}

/** What a chain's lookup gives for an address: the placement of an element the view shows
 * (`null`: shown, not placed), or `undefined` for one it doesn't show. */
export type PlacementLookup = (address: string) => PlacementJson | null | undefined;

/** The world matrix of `address` in the doc's space: `W(u) = W(relative_to) · P(u)`, `P(u)`
 * alone when `relative_to` is `null`. `null` when the chain is broken (an element with no
 * placement or an unknown one, an address not shown, a degenerate step, a cycle): a missing
 * link is never identity. */
export function worldMatrix(lookup: PlacementLookup, address: string, precision: Precision = "f64"): Mat4 | null {
  const chain: PlacementJson[] = [];
  const seen = new Set<string>();
  let at: string | null = address;
  while (at !== null) {
    if (seen.has(at)) return null;
    seen.add(at);
    const p = lookup(at);
    if (!p) return null;
    chain.push(p);
    at = p.relative_to ?? null;
  }
  let w = identity();
  for (let i = chain.length - 1; i >= 0; i--) {
    const m = placementMatrix(chain[i], precision);
    if (!m) return null;
    w = mul(w, m);
  }
  return w;
}

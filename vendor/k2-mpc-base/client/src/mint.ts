// Tagged-key minting, the kit's `crate::mint` (Rust) for clients: list keys two writers never
// mint alike. A tagged key is `<prefix><n>.<tag>`: `prefix` 1 to 3 of [a-z], `n` a decimal
// number without leading zeros, `tag` the writer's client ID in lower-case base 36. The whole
// key fits a list key's 24 bytes. Checked against the kit's vectors (test/vectors/mint.json).
//
// `n` is a bigint: it goes to 2^64 - 1, past a JavaScript number's exact range. A client ID
// is 53 bits, so a number holds it exactly.

/** Longest list key, in bytes. */
export const MAX_KEY_LEN = 24;
/** Longest key prefix. */
export const MAX_PREFIX_LEN = 3;
/** Longest writer tag: base 36 of 2^53 - 2. */
export const MAX_TAG_LEN = 11;
/** Largest valid client ID (2^53 - 1), the native engine's import agent's: never a writer. */
export const MAX_CLIENT_ID = 2 ** 53 - 1;
/** Largest writer's client ID (2^53 - 2). */
export const MAX_KIT_CLIENT = MAX_CLIENT_ID - 1;
const U64_MAX = (1n << 64n) - 1n;

/** Why a key can't be minted. */
export type MintError =
  | { kind: "bad_prefix"; prefix: string }
  | { kind: "too_long"; len: number }
  | { kind: "exhausted" };

/** The writer tag of `client`, or `null` when no doc accepts it as a writer (0, the session's
 * own 1, the import agent's 2^53 - 1). */
export function writerTag(client: number): string | null {
  if (!Number.isSafeInteger(client) || client < 2 || client > MAX_KIT_CLIENT) return null;
  return client.toString(36);
}

/** The client ID a tag spells: canonical base 36 only (no leading zero), of a writer. */
export function parseTag(s: string): number | null {
  if (s.length === 0 || s.length > MAX_TAG_LEN || s[0] === "0" || !/^[0-9a-z]+$/.test(s)) return null;
  const v = parseInt(s, 36);
  if (!Number.isSafeInteger(v) || v.toString(36) !== s) return null;
  return writerTag(v) === null ? null : v;
}

/** A key read as a tagged key. */
export interface TaggedKey {
  prefix: string;
  n: bigint;
  /** The writer's tag (base 36). */
  tag: string;
}

const goodPrefix = (p: string) => /^[a-z]{1,3}$/.test(p);

/** `key` as a tagged key, in its canonical spelling only; `null` otherwise. */
export function parseKey(key: string): TaggedKey | null {
  const m = /^([a-z]+)([0-9]+)\.(.*)$/.exec(key);
  if (!m) return null;
  const [, prefix, digits, tag] = m;
  if (!goodPrefix(prefix)) return null;
  if (digits.length > 1 && digits[0] === "0") return null;
  const n = BigInt(digits);
  if (n > U64_MAX) return null;
  if (parseTag(tag) === null) return null;
  return { prefix, n, tag };
}

/** The key `<prefix><n>.<tag>`, or why it can't be one. */
export function formatKey(prefix: string, n: bigint, tag: string): { key: string } | { error: MintError } {
  if (!goodPrefix(prefix)) return { error: { kind: "bad_prefix", prefix } };
  const key = `${prefix}${n}.${tag}`;
  if (key.length > MAX_KEY_LEN) return { error: { kind: "too_long", len: key.length } };
  return { key };
}

/** The next `n` for `prefix` and `tag`: one past the highest among `places` (every key the
 * list knows, live or deleted) with that prefix and tag, and the `retired` mark; at least 1. */
export function nextFor(places: Iterable<string>, retired: bigint, prefix: string, tag: string): { n: bigint } | { error: MintError } {
  if (!goodPrefix(prefix)) return { error: { kind: "bad_prefix", prefix } };
  let high = retired;
  for (const k of places) {
    const t = parseKey(k);
    if (t && t.prefix === prefix && t.tag === tag && t.n > high) high = t.n;
  }
  if (high >= U64_MAX) return { error: { kind: "exhausted" } };
  return { n: high + 1n };
}

/** One dropper's retired-mark key: `mint/<root>/<prefix>.<tag>/<dropper>`. */
export function markKey(root: string, prefix: string, tag: string, dropper: string): string {
  return `mint/${root}/${prefix}.${tag}/${dropper}`;
}

/** The retired mark for `prefix` and `tag` in list `root`: the max over every dropper's key
 * in `marks` (a pack's marks map); 0 for none. A value that isn't a decimal number is ignored. */
export function retiredMark(marks: Iterable<[string, string | null]>, root: string, prefix: string, tag: string): bigint {
  const base = `mint/${root}/${prefix}.${tag}`;
  let high = 0n;
  for (const [k, v] of marks) {
    if (!(k === base || k.startsWith(base + "/"))) continue;
    if (v === null || !/^[0-9]+$/.test(v)) continue;
    const n = BigInt(v);
    if (n <= U64_MAX && n > high) high = n;
  }
  return high;
}

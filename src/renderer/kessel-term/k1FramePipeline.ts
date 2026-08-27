// k1 binary apply pipeline (PRD grid-pause-snapshot-hitch G1).
//
// Full-scrollback decode must not run as a long sync job inside
// `ws.onmessage` (that turn also drives compose `onChange`). Peek the
// 3-byte kind, enqueue the ArrayBuffer, decode on the coalescer apply
// path, and ack the k1 floor only after the last applied chunk.

import {
  decodeGridFrame,
  peekK1Kind,
  type WireFrame,
} from './gridWire'

export type K1QueuedFrame =
  | { kind: 'snapshot'; buf: ArrayBuffer }
  | { kind: 'delta'; buf: ArrayBuffer }

/** Peek kind and wrap the raw buffer. Does not decode the grid. */
export function queueK1Binary(buf: ArrayBuffer): K1QueuedFrame {
  return { kind: peekK1Kind(buf), buf }
}

/** Decode one queued k1 buffer. Throws on malformed input. */
export function decodeK1Queued(frame: K1QueuedFrame): WireFrame {
  const decoded = decodeGridFrame(frame.buf)
  if (decoded.kind !== frame.kind) {
    throw new Error(
      `k1 wire decode error: peeked ${frame.kind} but decoded ${decoded.kind}`,
    )
  }
  return decoded
}

/** Decode a coalesced batch of raw k1 frames in apply order. */
export function decodeK1QueuedBatch(queued: K1QueuedFrame[]): WireFrame[] {
  return queued.map(decodeK1Queued)
}

export type AppliedVersionedFrame = { payload: { version: number } }

/** Highest applied version — ack only after the last applied chunk.
 *  Empty batch → 0 (caller must not send an ack). */
export function ackVersionAfterApply(
  applied: ReadonlyArray<AppliedVersionedFrame>,
): number {
  let max = 0
  for (const f of applied) {
    if (f.payload.version > max) max = f.payload.version
  }
  return max
}

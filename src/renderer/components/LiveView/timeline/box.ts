// React's dev build diffs a component's changed props a few levels deep for
// its performance track and JSON-serialises the short arrays of primitives
// it meets there. A transition's [in, out] is a pair of BigInt ticks, and
// JSON can't serialise a BigInt: the commit throws and the tab stops taking
// updates. So the model travels to components in a box with no enumerable
// keys (one box per model, so an unchanged model is the same prop).

export interface Box<T> {
  readonly v: T
}

const BOXES: WeakMap<WeakKey, Box<unknown>> = new WeakMap()

export function box<T extends object>(v: T): Box<T> {
  let b = BOXES.get(v)
  if (!b) {
    b = Object.defineProperty({}, 'v', { value: v, enumerable: false }) as Box<unknown>
    BOXES.set(v, b)
  }
  return b as Box<T>
}

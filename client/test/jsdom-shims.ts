// jsdom has no layout: CodeMirror's measuring asks ranges for their rectangles, which jsdom's
// Range lacks. Empty rectangles are enough for tests that never read positions on screen.

if (typeof Range !== "undefined") {
  const empty = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
  const zero = () => ({ x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0, toJSON() {} }) as DOMRect;
  if (!Range.prototype.getClientRects) Range.prototype.getClientRects = empty;
  if (!Range.prototype.getBoundingClientRect) Range.prototype.getBoundingClientRect = zero;
}

export {};

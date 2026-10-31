// Readable text on a participant's colour: black or white, whichever contrasts more (WCAG 2
// relative luminance). Colours the parser doesn't know get white.

function channels(colour: string): [number, number, number] | null {
  const c = colour.trim();
  let m = /^#([0-9a-f]{3})$/i.exec(c);
  if (m) return [...m[1]].map((x) => parseInt(x + x, 16)) as [number, number, number];
  m = /^#([0-9a-f]{6})(?:[0-9a-f]{2})?$/i.exec(c);
  if (m) return [0, 2, 4].map((i) => parseInt(m![1].slice(i, i + 2), 16)) as [number, number, number];
  m = /^rgba?\(\s*(\d+)[\s,]+(\d+)[\s,]+(\d+)/i.exec(c);
  if (m) return [Number(m[1]), Number(m[2]), Number(m[3])];
  return null;
}

/** WCAG relative luminance of an sRGB colour, 0 (black) to 1 (white). */
export function luminance(colour: string): number | null {
  const ch = channels(colour);
  if (!ch) return null;
  const [r, g, b] = ch.map((v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** The contrast ratio between two luminances (1 to 21). */
export function contrast(l1: number, l2: number): number {
  const [a, b] = l1 > l2 ? [l1, l2] : [l2, l1];
  return (a + 0.05) / (b + 0.05);
}

/** `#000000` or `#ffffff`, whichever reads better on `colour`. */
export function readableOn(colour: string): string {
  const l = luminance(colour);
  if (l === null) return "#ffffff";
  return contrast(l, 0) >= contrast(l, 1) ? "#000000" : "#ffffff";
}

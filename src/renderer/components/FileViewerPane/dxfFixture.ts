// Test fixture: a small ASCII DXF touching every drawn entity type, a
// block insert, a hidden layer and two types the viewer skips.

const g = (...pairs: (string | number)[]): string[] => pairs.map(String)

export const DXF_FIXTURE = [
  ...g(0, 'SECTION', 2, 'TABLES'),
  ...g(0, 'TABLE', 2, 'LAYER', 70, 2),
  ...g(0, 'LAYER', 2, 'WALLS', 70, 0, 62, 1, 6, 'CONTINUOUS'),
  ...g(0, 'LAYER', 2, 'HIDDEN', 70, 0, 62, -3, 6, 'CONTINUOUS'),
  ...g(0, 'ENDTAB'),
  ...g(0, 'ENDSEC'),
  ...g(0, 'SECTION', 2, 'BLOCKS'),
  ...g(0, 'BLOCK', 8, '0', 2, 'BOLT', 70, 0, 10, 0, 20, 0, 30, 0, 3, 'BOLT'),
  // ByBlock: takes the INSERT's colour.
  ...g(0, 'CIRCLE', 8, '0', 62, 0, 10, 0, 20, 0, 30, 0, 40, 2),
  ...g(0, 'ENDBLK', 8, '0'),
  ...g(0, 'ENDSEC'),
  ...g(0, 'SECTION', 2, 'ENTITIES'),
  ...g(0, 'LINE', 8, 'WALLS', 10, 0, 20, 0, 30, 0, 11, 100, 21, 0, 31, 0),
  ...g(0, 'CIRCLE', 8, '0', 62, 3, 10, 50, 20, 50, 30, 0, 40, 10),
  ...g(0, 'ARC', 8, '0', 10, 50, 20, 50, 30, 0, 40, 20, 50, 0, 51, 90),
  ...g(0, 'LWPOLYLINE', 8, '0', 90, 2, 70, 0, 10, 0, 20, 0, 42, 1, 10, 10, 20, 0),
  ...g(0, 'TEXT', 8, '0', 10, 0, 20, -10, 30, 0, 40, 5, 1, 'HELLO %%c10'),
  ...g(0, 'MTEXT', 8, '0', 10, 0, 20, 80, 30, 0, 40, 4, 71, 1, 1, '{\\fArial|b1;Top}\\PSecond'),
  ...g(0, 'ELLIPSE', 8, '0', 10, 0, 20, 0, 30, 0, 11, 10, 21, 0, 31, 0, 40, 0.5, 41, 0, 42, 6.283185307),
  ...g(0, 'INSERT', 8, 'WALLS', 62, 5, 2, 'BOLT', 10, 20, 20, 20, 30, 0),
  ...g(0, 'LINE', 8, 'HIDDEN', 10, 0, 20, 0, 30, 0, 11, 1, 21, 1, 31, 0),
  ...g(0, 'POINT', 8, '0', 10, 1, 20, 1, 30, 0),
  ...g(0, 'HATCH', 8, '0', 10, 0, 20, 0, 30, 0, 2, 'SOLID', 70, 1),
  ...g(0, 'ENDSEC'),
  ...g(0, 'EOF'),
].join('\n')

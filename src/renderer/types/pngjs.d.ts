// pngjs (a dev dependency): the little the headless WebKit render checks
// (`*.wktest.ts`) use to read pixels back from a screenshot.
declare module 'pngjs' {
  export class PNG {
    width: number
    height: number
    /** RGBA, 4 bytes a pixel, row by row. */
    data: Buffer
    static sync: {
      read(buffer: Buffer): PNG
      write(png: PNG): Buffer
    }
  }
}

export interface QrSvgOptions {
    /** The image's width and height in pixels (default: 8 per module). */
    size?: number;
    /** Light modules around the code (default 4, the QR standard). */
    quiet?: number;
    /** The dark color (default black). */
    dark?: string;
    /** The background (default white). */
    light?: string;
}
/**
 * An SVG string for a QR's module rows (`pairing.start`'s `qr.rows`).
 * Throws on rows that aren't a square of '0' and '1', or a color that
 * isn't a hex or named color (nothing from it reaches the markup unchecked).
 */
export declare function qrSvg(rows: readonly string[], opts?: QrSvgOptions): string;

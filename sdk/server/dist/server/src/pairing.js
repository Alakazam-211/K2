// Phone pairing helpers (prd-app-phone-pairing-v1 §8.4). Hand-written,
// beside the generated verbs: `pairing.start` answers the QR as rows of
// '1' (dark) and '0' (light) modules, and this draws them as an SVG string
// (no network, no dependency), for a web page or an email-free BFF.
//
//   const p = await k2.asGuest(pass).pairing.start()
//   res.send(qrSvg(p.qr.rows, { size: 240 }))
const COLOR = /^(#[0-9a-fA-F]{3,8}|[a-zA-Z]{1,32})$/;
/**
 * An SVG string for a QR's module rows (`pairing.start`'s `qr.rows`).
 * Throws on rows that aren't a square of '0' and '1', or a color that
 * isn't a hex or named color (nothing from it reaches the markup unchecked).
 */
export function qrSvg(rows, opts = {}) {
    const n = rows.length;
    if (n === 0 || rows.some((r) => r.length !== n || !/^[01]+$/.test(r))) {
        throw new Error('qrSvg: rows must be a square of 0 and 1 (pairing.start → qr.rows)');
    }
    const quiet = opts.quiet ?? 4;
    if (!Number.isInteger(quiet) || quiet < 0 || quiet > 16)
        throw new Error('qrSvg: quiet is a whole number from 0 to 16');
    const dark = opts.dark ?? '#000';
    const light = opts.light ?? '#fff';
    if (!COLOR.test(dark) || !COLOR.test(light))
        throw new Error('qrSvg: dark and light are hex (#000) or named colors');
    const view = n + 2 * quiet;
    const size = opts.size ?? view * 8;
    if (!Number.isFinite(size) || size <= 0)
        throw new Error('qrSvg: size is a positive number of pixels');
    let path = '';
    rows.forEach((row, y) => {
        for (let x = 0; x < n; x++) {
            if (row[x] === '1')
                path += `M${x + quiet} ${y + quiet}h1v1h-1z`;
        }
    });
    return (`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${view} ${view}" width="${size}" height="${size}" shape-rendering="crispEdges">` +
        `<rect width="${view}" height="${view}" fill="${light}"/>` +
        `<path d="${path}" fill="${dark}"/></svg>`);
}

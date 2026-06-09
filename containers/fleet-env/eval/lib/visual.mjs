// Visual diff / crop utilities (Track D, OPTIONAL — screenshot compare).
//
// Behaviours can use these to (a) capture a CROPPED region of the editor (e.g. just
// the status bar or activity bar) for tighter visual evidence, and (b) DIFF two PNG
// screenshots to detect "did anything visibly change" without eyeballing.
//
// Dependency-light by design: cropping uses Playwright's native screenshot clip /
// element-handle screenshot (no image decode). Comparison ships a tiny self-contained
// PNG reader (zlib is built into node) so we can do a real per-pixel diff without
// pulling in pngjs/sharp/pixelmatch. It handles the 8-bit non-interlaced PNGs that
// Playwright/Chromium emits (color types 2=RGB and 6=RGBA); anything else → it falls
// back to a cheap byte-length / identity comparison and flags `decoded:false`.

import { readFileSync, writeFileSync, existsSync } from "node:fs";
import zlib from "node:zlib";
import { OUT } from "./env.mjs";

// ─── Cropping ────────────────────────────────────────────────────────────────
/**
 * Screenshot just a rectangle of the page. Returns the file path.
 * @param {import("playwright").Page} page
 * @param {{x:number,y:number,width:number,height:number}} clip
 * @param {string} tag   filename tag → <OUT>/<tag>.png
 */
export async function cropRegion(page, clip, tag) {
  const path = `${OUT}/${tag}.png`;
  await page.screenshot({ path, clip });
  return path;
}

/**
 * Screenshot a DOM element (by CSS selector) — handy for VS Code chrome like the
 * status bar (`.statusbar`), activity bar (`.activitybar`) or a specific tab.
 * Returns the path, or null if the selector isn't present.
 * @param {import("playwright").Page} page
 * @param {string} selector
 * @param {string} tag
 */
export async function cropSelector(page, selector, tag) {
  const el = await page.$(selector);
  if (!el) return null;
  const path = `${OUT}/${tag}.png`;
  await el.screenshot({ path });
  return path;
}

// Common VS Code workbench regions, for convenience.
export const REGION = {
  statusBar: ".monaco-workbench .part.statusbar",
  activityBar: ".monaco-workbench .part.activitybar",
  sideBar: ".monaco-workbench .part.sidebar",
  panel: ".monaco-workbench .part.panel",
  editor: ".monaco-workbench .part.editor",
  titleBar: ".monaco-workbench .part.titlebar",
};

// ─── Comparison ──────────────────────────────────────────────────────────────
/**
 * Compare two PNG files. Returns:
 *   { changed:boolean, decoded:boolean, ratio:number, diffPixels:number,
 *     totalPixels:number, width, height, diffPath? }
 * `ratio` = fraction of differing pixels (0..1). `threshold` (default 0.001) sets
 * the per-channel tolerance × pixel-fraction below which we call it unchanged.
 * When both decode to the same dimensions and `diffPath` is given, writes a diff
 * mask PNG (changed pixels in magenta on black).
 *
 * @param {string} aPath
 * @param {string} bPath
 * @param {{ tolerance?:number, threshold?:number, diffTag?:string }} [opts]
 */
export function compareScreenshots(aPath, bPath, opts = {}) {
  const tolerance = opts.tolerance ?? 12;      // per-channel 0..255 abs diff to ignore (anti-alias)
  const threshold = opts.threshold ?? 0.001;   // fraction of pixels that must differ to count as "changed"

  if (!existsSync(aPath) || !existsSync(bPath)) {
    return { changed: false, decoded: false, ratio: 0, diffPixels: 0, totalPixels: 0,
             error: "missing screenshot file" };
  }

  const aBuf = readFileSync(aPath);
  const bBuf = readFileSync(bPath);

  let a, b;
  try { a = decodePNG(aBuf); b = decodePNG(bBuf); }
  catch { return byteFallback(aBuf, bBuf); }

  if (!a || !b) return byteFallback(aBuf, bBuf);
  if (a.width !== b.width || a.height !== b.height) {
    // Different dimensions ⇒ definitely changed; can't pixel-diff.
    return { changed: true, decoded: true, ratio: 1, diffPixels: a.width * a.height,
             totalPixels: a.width * a.height, width: a.width, height: a.height,
             note: `dimension change ${a.width}x${a.height} → ${b.width}x${b.height}` };
  }

  const total = a.width * a.height;
  let diff = 0;
  const mask = opts.diffTag ? Buffer.alloc(total * 4, 0) : null;
  for (let i = 0; i < total; i++) {
    const o = i * 4;
    const dr = Math.abs(a.rgba[o] - b.rgba[o]);
    const dg = Math.abs(a.rgba[o + 1] - b.rgba[o + 1]);
    const db = Math.abs(a.rgba[o + 2] - b.rgba[o + 2]);
    const da = Math.abs(a.rgba[o + 3] - b.rgba[o + 3]);
    if (dr > tolerance || dg > tolerance || db > tolerance || da > tolerance) {
      diff++;
      if (mask) { mask[o] = 255; mask[o + 1] = 0; mask[o + 2] = 255; mask[o + 3] = 255; }
    }
  }

  const ratio = total ? diff / total : 0;
  const out = { changed: ratio > threshold, decoded: true, ratio: round4(ratio),
                diffPixels: diff, totalPixels: total, width: a.width, height: a.height };

  if (mask && opts.diffTag) {
    const diffPath = `${OUT}/${opts.diffTag}.png`;
    try { writeFileSync(diffPath, encodePNG(a.width, a.height, mask)); out.diffPath = diffPath; }
    catch { /* diff image is best-effort */ }
  }
  return out;
}

// When we can't decode (interlaced / palette / 16-bit), fall back to a byte compare:
// identical bytes ⇒ definitely unchanged; otherwise we can only say "changed".
function byteFallback(aBuf, bBuf) {
  const same = aBuf.length === bBuf.length && aBuf.equals(bBuf);
  return { changed: !same, decoded: false, ratio: same ? 0 : 1,
           diffPixels: same ? 0 : -1, totalPixels: -1,
           note: "byte-level comparison (PNG not decoded)" };
}

// ─── Minimal PNG codec (8-bit, non-interlaced, color type 2/6) ─────────────────
const PNG_SIG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

/** Decode a PNG buffer → {width,height,rgba:Buffer(w*h*4)} or null if unsupported. */
export function decodePNG(buf) {
  if (buf.length < 8 || !buf.subarray(0, 8).equals(PNG_SIG)) return null;
  let pos = 8;
  let width = 0, height = 0, bitDepth = 0, colorType = 0, interlace = 0;
  const idat = [];
  while (pos + 8 <= buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("ascii", pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
      interlace = data[12];
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    pos += 12 + len; // length + type + data + crc
  }
  if (!width || !height) return null;
  if (bitDepth !== 8 || interlace !== 0) return null;       // unsupported variant
  if (colorType !== 2 && colorType !== 6) return null;       // need RGB or RGBA
  const channels = colorType === 6 ? 4 : 3;

  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const rgba = Buffer.alloc(width * height * 4);
  const prev = Buffer.alloc(stride);
  let inPos = 0;
  for (let y = 0; y < height; y++) {
    const filter = raw[inPos++];
    const line = Buffer.from(raw.subarray(inPos, inPos + stride));
    inPos += stride;
    unfilter(filter, line, prev, channels);
    line.copy(prev);
    for (let x = 0; x < width; x++) {
      const si = x * channels, di = (y * width + x) * 4;
      rgba[di] = line[si];
      rgba[di + 1] = line[si + 1];
      rgba[di + 2] = line[si + 2];
      rgba[di + 3] = channels === 4 ? line[si + 3] : 255;
    }
  }
  return { width, height, rgba };
}

// Reverse the PNG scanline filters (0 None, 1 Sub, 2 Up, 3 Average, 4 Paeth).
function unfilter(filter, line, prev, bpp) {
  const len = line.length;
  for (let i = 0; i < len; i++) {
    const a = i >= bpp ? line[i - bpp] : 0;   // left
    const b = prev[i];                         // up
    const c = i >= bpp ? prev[i - bpp] : 0;    // up-left
    let val = line[i];
    switch (filter) {
      case 1: val += a; break;
      case 2: val += b; break;
      case 3: val += (a + b) >> 1; break;
      case 4: val += paeth(a, b, c); break;
      default: break; // 0 None
    }
    line[i] = val & 0xff;
  }
}

function paeth(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
  if (pa <= pb && pa <= pc) return a;
  if (pb <= pc) return b;
  return c;
}

/** Encode RGBA → a (deflated, filter-0) PNG buffer. Used for the diff mask. */
export function encodePNG(width, height, rgba) {
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0; // filter None
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride);
  }
  const idat = zlib.deflateSync(raw);
  const chunks = [
    PNG_SIG,
    chunk("IHDR", ihdr(width, height)),
    chunk("IDAT", idat),
    chunk("IEND", Buffer.alloc(0)),
  ];
  return Buffer.concat(chunks);
}

function ihdr(width, height) {
  const b = Buffer.alloc(13);
  b.writeUInt32BE(width, 0);
  b.writeUInt32BE(height, 4);
  b[8] = 8;   // bit depth
  b[9] = 6;   // color type RGBA
  b[10] = 0;  // compression
  b[11] = 0;  // filter
  b[12] = 0;  // interlace
  return b;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const crcBuf = Buffer.alloc(4);
  crcBuf.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])) >>> 0, 0);
  return Buffer.concat([len, typeBuf, data, crcBuf]);
}

// CRC-32 (PNG polynomial) — small table built once.
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function round4(n) { return Math.round(n * 1e4) / 1e4; }

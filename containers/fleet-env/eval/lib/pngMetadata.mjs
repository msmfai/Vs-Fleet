import { inflateSync } from "node:zlib";
import { readFileSync, writeFileSync } from "node:fs";

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const CRC_TABLE = new Uint32Array(256);

for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
  CRC_TABLE[n] = c >>> 0;
}

export function readPngText(path) {
  const chunks = parsePng(readFileSync(path));
  const text = {};
  for (const chunk of chunks) {
    const entry = decodeTextChunk(chunk);
    if (entry) text[entry.keyword] = entry.text;
  }
  return text;
}

export function upsertPngTexts(path, entries) {
  const source = readFileSync(path);
  const chunks = parsePng(source);
  const keywords = new Set(entries.map((entry) => entry.keyword));
  const next = [];
  let inserted = false;

  for (const chunk of chunks) {
    const existing = decodeTextKeyword(chunk);
    if (existing && keywords.has(existing)) continue;
    if (!inserted && chunk.type === "IEND") {
      for (const entry of entries) next.push(makeItxtChunk(entry.keyword, entry.text));
      inserted = true;
    }
    next.push(serializeChunk(chunk));
  }

  if (!inserted) throw new Error(`PNG missing IEND chunk: ${path}`);
  writeFileSync(path, Buffer.concat([PNG_SIGNATURE, ...next]));
}

function parsePng(buf) {
  if (buf.length < PNG_SIGNATURE.length || !buf.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)) {
    throw new Error("not a PNG file");
  }
  const chunks = [];
  let off = PNG_SIGNATURE.length;
  while (off + 12 <= buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.subarray(off + 4, off + 8).toString("ascii");
    const dataStart = off + 8;
    const dataEnd = dataStart + len;
    const crcEnd = dataEnd + 4;
    if (crcEnd > buf.length) throw new Error(`truncated PNG chunk ${type}`);
    chunks.push({
      type,
      data: buf.subarray(dataStart, dataEnd),
      crc: buf.readUInt32BE(dataEnd),
    });
    off = crcEnd;
    if (type === "IEND") break;
  }
  return chunks;
}

function serializeChunk(chunk) {
  const type = Buffer.from(chunk.type, "ascii");
  const out = Buffer.alloc(12 + chunk.data.length);
  out.writeUInt32BE(chunk.data.length, 0);
  type.copy(out, 4);
  chunk.data.copy(out, 8);
  out.writeUInt32BE(chunk.crc ?? crc32(Buffer.concat([type, chunk.data])), 8 + chunk.data.length);
  return out;
}

function makeItxtChunk(keyword, text) {
  if (!/^[\x20-\x7e]{1,79}$/.test(keyword) || keyword.includes("\0")) {
    throw new Error(`invalid PNG text keyword: ${keyword}`);
  }
  const data = Buffer.concat([
    Buffer.from(keyword, "latin1"),
    Buffer.from([0, 0, 0, 0, 0]),
    Buffer.from(String(text), "utf8"),
  ]);
  const type = Buffer.from("iTXt", "ascii");
  return serializeChunk({ type: "iTXt", data, crc: crc32(Buffer.concat([type, data])) });
}

function decodeTextKeyword(chunk) {
  if (!["tEXt", "zTXt", "iTXt"].includes(chunk.type)) return null;
  const nul = chunk.data.indexOf(0);
  if (nul <= 0) return null;
  return chunk.data.subarray(0, nul).toString("latin1");
}

function decodeTextChunk(chunk) {
  const keyword = decodeTextKeyword(chunk);
  if (!keyword) return null;

  if (chunk.type === "tEXt") {
    return { keyword, text: chunk.data.subarray(keyword.length + 1).toString("latin1") };
  }
  if (chunk.type === "zTXt") {
    const methodOffset = keyword.length + 1;
    if (chunk.data[methodOffset] !== 0) return null;
    return { keyword, text: inflateSync(chunk.data.subarray(methodOffset + 1)).toString("latin1") };
  }
  if (chunk.type !== "iTXt") return null;

  let off = keyword.length + 1;
  const compressed = chunk.data[off++] === 1;
  const compressionMethod = chunk.data[off++];
  const langEnd = chunk.data.indexOf(0, off);
  if (langEnd < 0) return null;
  off = langEnd + 1;
  const translatedEnd = chunk.data.indexOf(0, off);
  if (translatedEnd < 0) return null;
  off = translatedEnd + 1;
  const textBuf = chunk.data.subarray(off);
  if (!compressed) return { keyword, text: textBuf.toString("utf8") };
  if (compressionMethod !== 0) return null;
  return { keyword, text: inflateSync(textBuf).toString("utf8") };
}

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

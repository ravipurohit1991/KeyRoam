// Renders the KeyRoam key icon as PNGs (no dependencies): a white key on a
// rounded blue square. Usage: node tools/make-icons.mjs [outDir] [sizes...]

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';

const here = dirname(fileURLToPath(import.meta.url));
const outDir = process.argv[2] || join(here, '..', 'src', 'icons');
const sizes = process.argv.length > 3 ? process.argv.slice(3).map(Number) : [16, 32, 48, 128];

const BG = [49, 87, 213];
const FG = [255, 255, 255];

// Signed distance helpers in a 0..1 unit square (negative = inside).
const roundRect = (x, y, cx, cy, hw, hh, r) => {
  const qx = Math.abs(x - cx) - hw + r;
  const qy = Math.abs(y - cy) - hh + r;
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r;
};
const circle = (x, y, cx, cy, r) => Math.hypot(x - cx, y - cy) - r;
const box = (x, y, x0, y0, x1, y1) => roundRect(x, y, (x0 + x1) / 2, (y0 + y1) / 2, (x1 - x0) / 2, (y1 - y0) / 2, 0.01);

function key(x, y) {
  const ring = Math.max(circle(x, y, 0.34, 0.5, 0.17), -circle(x, y, 0.34, 0.5, 0.075));
  const shaft = box(x, y, 0.46, 0.455, 0.82, 0.545);
  const tooth1 = box(x, y, 0.66, 0.545, 0.72, 0.65);
  const tooth2 = box(x, y, 0.76, 0.545, 0.82, 0.69);
  return Math.min(ring, shaft, tooth1, tooth2);
}

function pixel(x, y) {
  const inBg = roundRect(x, y, 0.5, 0.5, 0.5, 0.5, 0.22) <= 0;
  if (!inBg) return [0, 0, 0, 0];
  return key(x, y) <= 0 ? [...FG, 255] : [...BG, 255];
}

function render(size) {
  const ss = 4; // supersampling per axis
  const rgba = Buffer.alloc(size * size * 4);
  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      const acc = [0, 0, 0, 0];
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const [r, g, b, a] = pixel((px + (sx + 0.5) / ss) / size, (py + (sy + 0.5) / ss) / size);
          acc[0] += r * a;
          acc[1] += g * a;
          acc[2] += b * a;
          acc[3] += a;
        }
      }
      const o = (py * size + px) * 4;
      const a = acc[3] / (ss * ss);
      rgba[o] = acc[3] ? Math.round(acc[0] / acc[3]) : 0;
      rgba[o + 1] = acc[3] ? Math.round(acc[1] / acc[3]) : 0;
      rgba[o + 2] = acc[3] ? Math.round(acc[2] / acc[3]) : 0;
      rgba[o + 3] = Math.round(a);
    }
  }
  return png(size, size, rgba);
}

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}
function png(w, h, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) rgba.copy(raw, y * (w * 4 + 1) + 1, y * w * 4, (y + 1) * w * 4);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

mkdirSync(outDir, { recursive: true });
for (const size of sizes) {
  const file = join(outDir, `icon-${size}.png`);
  writeFileSync(file, render(size));
  console.log(`wrote ${file}`);
}

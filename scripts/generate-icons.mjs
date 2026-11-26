#!/usr/bin/env node
/**
 * Gera os ícones do aplicativo desktop.
 *
 * O repositório não depende de ImageMagick nem de Inkscape: os PNGs são
 * rasterizados aqui, com supersampling 3x3, e escritos com um encoder mínimo
 * (`node:zlib` + CRC32). Rodar de novo é idempotente.
 *
 *   node scripts/generate-icons.mjs
 *
 * A marca é um núcleo luminoso cercado por dois anéis parciais: o runtime no
 * centro, os agentes orbitando.
 */
import { deflateSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const OUT_DIR = join(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  'apps',
  'desktop',
  'src-tauri',
  'icons',
);

const SIZES = [
  ['32x32.png', 32],
  ['128x128.png', 128],
  ['128x128@2x.png', 256],
  ['icon.png', 512],
];

const SS = 3; // fator de supersampling por eixo

// --- paleta -----------------------------------------------------------------

const BG_TOP = [0x0a, 0x0f, 0x1c];
const BG_BOTTOM = [0x15, 0x1f, 0x38];
const CORE_HOT = [0x7d, 0xe2, 0xff];
const CORE_COOL = [0x8b, 0x7c, 0xf6];
const RING = [0x5e, 0xea, 0xd4];

const mix = (a, b, t) => [
  Math.round(a[0] + (b[0] - a[0]) * t),
  Math.round(a[1] + (b[1] - a[1]) * t),
  Math.round(a[2] + (b[2] - a[2]) * t),
];

/** Distância assinada a um retângulo de cantos arredondados centrado na origem. */
function roundedBoxSdf(x, y, half, radius) {
  const qx = Math.abs(x) - half + radius;
  const qy = Math.abs(y) - half + radius;
  const outside = Math.hypot(Math.max(qx, 0), Math.max(qy, 0));
  return outside + Math.min(Math.max(qx, qy), 0) - radius;
}

/** Cor da amostra em coordenadas normalizadas (-0.5 .. 0.5). */
function shade(nx, ny) {
  const boxD = roundedBoxSdf(nx, ny, 0.5, 0.22);
  if (boxD > 0) return [0, 0, 0, 0];

  // Fundo: gradiente diagonal suave.
  const t = (nx + ny + 1) / 2;
  let [r, g, b] = mix(BG_TOP, BG_BOTTOM, Math.min(Math.max(t, 0), 1));

  const dist = Math.hypot(nx, ny);

  // Halo do núcleo.
  const halo = Math.exp(-((dist / 0.2) ** 2)) * 0.55;
  const haloColor = mix(CORE_COOL, CORE_HOT, 0.5);
  r += (haloColor[0] - r) * halo;
  g += (haloColor[1] - g) * halo;
  b += (haloColor[2] - b) * halo;

  // Núcleo sólido.
  if (dist < 0.135) {
    const k = Math.min(Math.max((0.135 - dist) / 0.135, 0), 1);
    const core = mix(CORE_COOL, CORE_HOT, Math.pow(k, 0.6));
    const blend = Math.min(1, k * 6);
    r += (core[0] - r) * blend;
    g += (core[1] - g) * blend;
    b += (core[2] - b) * blend;
  }

  // Dois anéis parciais, com aberturas em lados opostos.
  const angle = Math.atan2(ny, nx);
  const rings = [
    { radius: 0.255, width: 0.016, gapAt: -Math.PI / 2, gapArc: 1.15, alpha: 0.9 },
    { radius: 0.355, width: 0.013, gapAt: Math.PI / 2, gapArc: 1.5, alpha: 0.5 },
  ];
  for (const ring of rings) {
    const band = Math.abs(dist - ring.radius);
    if (band > ring.width) continue;
    let delta = Math.abs(angle - ring.gapAt);
    if (delta > Math.PI) delta = 2 * Math.PI - delta;
    if (delta < ring.gapArc / 2) continue;
    const edge = 1 - band / ring.width;
    const a = ring.alpha * Math.min(1, edge * 2.2);
    r += (RING[0] - r) * a;
    g += (RING[1] - g) * a;
    b += (RING[2] - b) * a;
  }

  return [Math.round(r), Math.round(g), Math.round(b), 255];
}

/** Rasteriza um PNG RGBA quadrado do tamanho pedido. */
function render(size) {
  const pixels = Buffer.alloc(size * size * 4);
  const inv = 1 / (size * SS);
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;
      for (let sy = 0; sy < SS; sy += 1) {
        for (let sx = 0; sx < SS; sx += 1) {
          const nx = (x * SS + sx + 0.5) * inv - 0.5;
          const ny = (y * SS + sy + 0.5) * inv - 0.5;
          const s = shade(nx, ny);
          // Pré-multiplica para que a borda arredondada não escureça.
          const sa = s[3] / 255;
          r += s[0] * sa;
          g += s[1] * sa;
          b += s[2] * sa;
          a += s[3];
        }
      }
      const n = SS * SS;
      const alpha = a / n;
      const norm = alpha > 0 ? 255 / alpha : 0;
      const i = (y * size + x) * 4;
      pixels[i] = Math.min(255, Math.round((r / n) * norm));
      pixels[i + 1] = Math.min(255, Math.round((g / n) * norm));
      pixels[i + 2] = Math.min(255, Math.round((b / n) * norm));
      pixels[i + 3] = Math.round(alpha);
    }
  }
  return encodePng(size, size, pixels);
}

// --- encoder PNG ------------------------------------------------------------

const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  return table;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
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

function encodePng(width, height, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  // 10..12: compression, filter, interlace — todos 0.

  // Cada scanline recebe o byte de filtro 0 (None).
  const raw = Buffer.alloc(height * (width * 4 + 1));
  for (let y = 0; y < height; y += 1) {
    const dst = y * (width * 4 + 1);
    raw[dst] = 0;
    rgba.copy(raw, dst + 1, y * width * 4, (y + 1) * width * 4);
  }

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// --- main -------------------------------------------------------------------

mkdirSync(OUT_DIR, { recursive: true });
for (const [name, size] of SIZES) {
  const png = render(size);
  writeFileSync(join(OUT_DIR, name), png);
  console.log(`  ${name.padEnd(16)} ${size}×${size}  ${(png.length / 1024).toFixed(1)} KiB`);
}
console.log(`\nÍcones gerados em ${OUT_DIR}`);

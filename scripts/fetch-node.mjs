#!/usr/bin/env node
// Fetch the Node.js runtime the Linux and Windows apps bundle for their agent engine
// (daemon), so users don't need Node installed. Runs after the daemon build
// (daemon/package.json "postbuild"), before the Tauri build compiles.
//
// Pinned to one version with its published SHA-256 (nodejs.org
// SHASUMS256.txt), so a build always ships exactly this runtime or fails.
// Output: daemon/runtime/node/bin/node (node.exe on Windows), which
// tauri.linux.conf.json / tauri.windows.conf.json bundle as the app resource
// node/bin/node(.exe); supervisor.rs prefers it.
//
// macOS doesn't bundle Node yet: the runtime would need signing with JIT
// entitlements for notarization. macOS targets skip.
// Target: TAURI_ENV_TARGET_TRIPLE or AYGENT_NODE_TARGET, else this machine.

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import zlib from 'node:zlib';

const VERSION = 'v24.21.0';
const BUILDS = {
  'x86_64-unknown-linux-gnu': ['linux-x64', 'fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6'],
  'aarch64-unknown-linux-gnu': ['linux-arm64', '6ad1325edbdb5649c379b75a237147a666c95d4f9ae8d340fef2d1575d289ad2'],
  'x86_64-pc-windows-msvc': ['win-x64', '158f7685b44de51f6c0df1d153526cbcd3e1bc739a8dfc607721cef75de9e541'],
  'aarch64-pc-windows-msvc': ['win-arm64', '8779b1bde1d39f8d420e3b57aa657b39891af434d3de44a919044cec06785921'],
};

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const outDir = path.join(root, 'daemon', 'runtime', 'node', 'bin');
let out = path.join(outDir, 'node');
const stamp = path.join(outDir, '.version');

function hostTriple() {
  const cpu = process.arch === 'arm64' ? 'aarch64' : process.arch === 'x64' ? 'x86_64' : process.arch;
  if (process.platform === 'linux') return `${cpu}-unknown-linux-gnu`;
  if (process.platform === 'win32') return `${cpu}-pc-windows-msvc`;
  return `${cpu}-${process.platform}`;
}

const target = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.AYGENT_NODE_TARGET || hostTriple();
const windows = target.includes('windows');
if (windows) out = path.join(outDir, 'node.exe');
if (!target.includes('linux') && !windows) {
  console.log(`[fetch-node] ${target}: Node is not bundled on this platform; skipping`);
  process.exit(0);
}
const build = BUILDS[target];
if (!build) {
  console.error(`[fetch-node] no pinned Node build for ${target}`);
  process.exit(1);
}
const [suffix, sha256] = build;
const name = `node-${VERSION}-${suffix}`;

if (fs.existsSync(out) && fs.existsSync(stamp) && fs.readFileSync(stamp, 'utf8').trim() === name) {
  console.log(`[fetch-node] ${name} already in place`);
  process.exit(0);
}

const ext = windows ? 'zip' : 'tar.xz';
const url = `https://nodejs.org/dist/${VERSION}/${name}.${ext}`;
console.log(`[fetch-node] downloading ${url}`);
const res = await fetch(url);
if (!res.ok) {
  console.error(`[fetch-node] download failed: HTTP ${res.status}`);
  process.exit(1);
}
const data = Buffer.from(await res.arrayBuffer());
const got = createHash('sha256').update(data).digest('hex');
if (got !== sha256) {
  console.error(`[fetch-node] checksum mismatch for ${name}.${ext}: expected ${sha256}, got ${got}`);
  process.exit(1);
}

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'aygent-node-'));
try {
  const archive = path.join(tmp, `${name}.${ext}`);
  fs.writeFileSync(archive, data);
  fs.mkdirSync(outDir, { recursive: true });
  if (windows) {
    // One file out of the zip, read directly: whichever `tar` a build
    // shell finds first (GNU tar in Git Bash) may not read zip archives.
    fs.writeFileSync(out, unzipOne(data, `${name}/node.exe`));
  } else {
    execFileSync('tar', ['-xJf', archive, '-C', tmp, `${name}/bin/node`]);
    fs.copyFileSync(path.join(tmp, name, 'bin', 'node'), out);
  }
  fs.chmodSync(out, 0o755);
  fs.writeFileSync(stamp, `${name}\n`);
  console.log(`[fetch-node] ${name} -> ${path.relative(root, out)} (${(fs.statSync(out).size / 1048576).toFixed(0)} MB, sha256 verified)`);
} finally {
  fs.rmSync(tmp, { recursive: true, force: true });
}

// The bytes of one entry of a zip archive (stored or deflated).
function unzipOne(zip, wanted) {
  let eocd = zip.length - 22;
  while (eocd >= 0 && zip.readUInt32LE(eocd) !== 0x06054b50) eocd--;
  if (eocd < 0) throw new Error('not a zip archive');
  const count = zip.readUInt16LE(eocd + 10);
  let at = zip.readUInt32LE(eocd + 16); // central directory offset
  for (let i = 0; i < count; i++) {
    if (zip.readUInt32LE(at) !== 0x02014b50) throw new Error('bad zip central directory');
    const method = zip.readUInt16LE(at + 10);
    const size = zip.readUInt32LE(at + 20);
    const nameLen = zip.readUInt16LE(at + 28);
    const extraLen = zip.readUInt16LE(at + 30);
    const commentLen = zip.readUInt16LE(at + 32);
    const local = zip.readUInt32LE(at + 42);
    const entry = zip.toString('utf8', at + 46, at + 46 + nameLen);
    if (entry === wanted) {
      const start = local + 30 + zip.readUInt16LE(local + 26) + zip.readUInt16LE(local + 28);
      const body = zip.subarray(start, start + size);
      if (method === 0) return body;
      if (method === 8) return zlib.inflateRawSync(body);
      throw new Error(`unsupported zip compression method ${method}`);
    }
    at += 46 + nameLen + extraLen + commentLen;
  }
  throw new Error(`${wanted} not found in the archive`);
}

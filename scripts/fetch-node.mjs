#!/usr/bin/env node
// Fetch the Node.js runtime the Linux app bundles for its agent engine
// (daemon), so users don't need Node installed. Runs after the daemon build
// (daemon/package.json "postbuild"), before the Tauri build compiles.
//
// Pinned to one version with its published SHA-256 (nodejs.org
// SHASUMS256.txt), so a build always ships exactly this runtime or fails.
// Output: daemon/runtime/node/bin/node (tauri.linux.conf.json bundles it as
// the app resource node/bin/node; supervisor.rs prefers it).
//
// Only Linux targets bundle Node for now: a macOS bundle would need the
// runtime signed with JIT entitlements for notarization. Other targets skip.
// Target: TAURI_ENV_TARGET_TRIPLE or AYGENT_NODE_TARGET, else this machine.

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const VERSION = 'v24.21.0';
const BUILDS = {
  'x86_64-unknown-linux-gnu': ['linux-x64', 'fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6'],
  'aarch64-unknown-linux-gnu': ['linux-arm64', '6ad1325edbdb5649c379b75a237147a666c95d4f9ae8d340fef2d1575d289ad2'],
};

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const outDir = path.join(root, 'daemon', 'runtime', 'node', 'bin');
const out = path.join(outDir, 'node');
const stamp = path.join(outDir, '.version');

function hostTriple() {
  if (process.platform !== 'linux') return `${process.arch}-${process.platform}`;
  return process.arch === 'arm64' ? 'aarch64-unknown-linux-gnu' : process.arch === 'x64' ? 'x86_64-unknown-linux-gnu' : `${process.arch}-linux`;
}

const target = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.AYGENT_NODE_TARGET || hostTriple();
if (!target.includes('linux')) {
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

const url = `https://nodejs.org/dist/${VERSION}/${name}.tar.xz`;
console.log(`[fetch-node] downloading ${url}`);
const res = await fetch(url);
if (!res.ok) {
  console.error(`[fetch-node] download failed: HTTP ${res.status}`);
  process.exit(1);
}
const data = Buffer.from(await res.arrayBuffer());
const got = createHash('sha256').update(data).digest('hex');
if (got !== sha256) {
  console.error(`[fetch-node] checksum mismatch for ${name}.tar.xz: expected ${sha256}, got ${got}`);
  process.exit(1);
}

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'aygent-node-'));
try {
  const archive = path.join(tmp, `${name}.tar.xz`);
  fs.writeFileSync(archive, data);
  execFileSync('tar', ['-xJf', archive, '-C', tmp, `${name}/bin/node`]);
  fs.mkdirSync(outDir, { recursive: true });
  fs.copyFileSync(path.join(tmp, name, 'bin', 'node'), out);
  fs.chmodSync(out, 0o755);
  fs.writeFileSync(stamp, `${name}\n`);
  console.log(`[fetch-node] ${name} -> ${path.relative(root, out)} (${(fs.statSync(out).size / 1048576).toFixed(0)} MB, sha256 verified)`);
} finally {
  fs.rmSync(tmp, { recursive: true, force: true });
}

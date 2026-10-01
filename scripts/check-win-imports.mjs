#!/usr/bin/env node
// Fail if a Windows executable imports the Visual C++ runtime DLLs, which a
// clean Windows doesn't have (the app must link the CRT statically; see
// src-tauri/.cargo/config.toml). Usage: check-win-imports.mjs <file.exe>...

import fs from 'node:fs';

const RUNTIME = /^(msvcp\d+|vcruntime\d+(_\d+)?|concrt\d+|vcomp\d+|ucrtbased)\.dll$/i;

// The DLLs a PE file imports, normally and delay-loaded.
function imports(file) {
  const b = fs.readFileSync(file);
  const pe = b.readUInt32LE(0x3c);
  if (b.toString('latin1', pe, pe + 4) !== 'PE\0\0') throw new Error(`${file}: not a PE file`);
  const sections = b.readUInt16LE(pe + 6);
  const opt = pe + 24;
  const dirs = opt + (b.readUInt16LE(opt) === 0x20b ? 112 : 96);
  const table = opt + b.readUInt16LE(pe + 20);
  const offset = (rva) => {
    for (let i = 0; i < sections; i++) {
      const s = table + i * 40;
      const va = b.readUInt32LE(s + 12);
      const size = Math.max(b.readUInt32LE(s + 8), b.readUInt32LE(s + 16));
      if (rva >= va && rva < va + size) return rva - va + b.readUInt32LE(s + 20);
    }
    throw new Error(`${file}: RVA ${rva} is in no section`);
  };
  const cstr = (rva) => {
    const at = offset(rva);
    return b.toString('latin1', at, b.indexOf(0, at));
  };
  const names = [];
  // [directory index, descriptor size, name field offset]
  for (const [index, size, nameAt] of [[1, 20, 12], [13, 32, 4]]) {
    const rva = b.readUInt32LE(dirs + index * 8);
    if (!rva) continue;
    for (let d = offset(rva); ; d += size) {
      const name = b.readUInt32LE(d + nameAt);
      if (!name) break;
      names.push(cstr(name));
    }
  }
  return names;
}

let bad = false;
for (const file of process.argv.slice(2)) {
  const dlls = imports(file);
  const runtime = dlls.filter((d) => RUNTIME.test(d));
  console.log(`${file}: ${dlls.join(', ')}`);
  if (runtime.length) {
    console.error(`${file} needs the Visual C++ runtime (${runtime.join(', ')}); link it statically`);
    bad = true;
  }
}
process.exit(bad ? 1 : 0);

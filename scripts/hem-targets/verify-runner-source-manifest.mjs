#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This runner source verifier is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';

const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const comparePath = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const fail = message => { throw new Error(message); };

async function inventory(root) {
  const found = [];
  async function visit(relative) {
    for (const entry of await fs.readdir(path.join(root, relative), { withFileTypes: true })) {
      const child = relative ? `${relative}/${entry.name}` : entry.name;
      if (!relative && child === '.git') continue;
      if (!relative && child === 'runner-source.json') continue;
      if (entry.isSymbolicLink()) fail(`Symlink in runner source tree: ${child}`);
      if (entry.isDirectory()) await visit(child);
      else if (entry.isFile()) found.push({ path: child, sha256: sha(await fs.readFile(path.join(root, child))) });
      else fail(`Non-regular file in runner source tree: ${child}`);
    }
  }
  await visit('');
  return found.sort((a, b) => comparePath(a.path, b.path));
}

export async function verifyRunnerSourceManifest({ root, manifestPath, phase = 'before-build' }) {
  if (!['before-build', 'after-build'].includes(phase)) fail(`Unknown verification phase: ${phase}`);
  const manifest = JSON.parse(await fs.readFile(manifestPath, 'utf8'));
  if (manifest.schemaVersion !== 1 || !/^[a-f0-9]{40}$/.test(manifest.sourceCommit ?? '') ||
      !/^[a-f0-9]{64}$/.test(manifest.sourceTreeSha256 ?? '') || typeof manifest.sourceTreeDirty !== 'boolean' ||
      !/^[a-f0-9]{64}$/.test(manifest.exportFilesSha256 ?? '') || !Array.isArray(manifest.files)) {
    fail('Invalid runner source manifest header');
  }
  const expected = manifest.files;
  let previous = '';
  for (const item of expected) {
    if (!item || typeof item.path !== 'string' || !/^[a-f0-9]{64}$/.test(item.sha256 ?? '') ||
        item.path.startsWith('/') || item.path.includes('\\') || item.path.split('/').some(part => !part || part === '.' || part === '..')) {
      fail(`Invalid runner source manifest file entry: ${item?.path}`);
    }
    if (previous && item.path <= previous) fail(`Runner source manifest paths are not unique and sorted at ${item.path}`);
    previous = item.path;
  }
  const computedExportHash = sha(expected.map(item => `${item.path}\0${item.sha256}\n`).join(''));
  if (computedExportHash !== manifest.exportFilesSha256) fail('Runner source manifest inventory digest mismatch');
  if (manifest.sourceTreeSha256 !== manifest.exportFilesSha256) fail('Runner source tree identity does not match exported file inventory');
  const actual = await inventory(path.resolve(root));
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    const actualByPath = new Map(actual.map(item => [item.path, item.sha256]));
    const expectedByPath = new Map(expected.map(item => [item.path, item.sha256]));
    const changed = [...new Set([...actualByPath.keys(), ...expectedByPath.keys()])].sort()
      .filter(file => actualByPath.get(file) !== expectedByPath.get(file));
    fail(`Runner source changed ${phase}: ${changed.slice(0, 12).join(', ')}`);
  }
  return { phase, files: actual.length, exportFilesSha256: computedExportHash,
    sourceCommit: manifest.sourceCommit, sourceTreeSha256: manifest.sourceTreeSha256,
    sourceTreeDirty: manifest.sourceTreeDirty };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const args = process.argv.slice(2);
  const read = name => { const at = args.indexOf(name); return at < 0 ? undefined : args[at + 1]; };
  const manifest = read('--manifest'), root = read('--root') ?? '.', phase = read('--phase') ?? 'before-build';
  if (!manifest || args.some((arg, i) => arg.startsWith('--') && !['--manifest', '--root', '--phase'].includes(arg))) {
    throw new Error('Usage: node scripts/hem-targets/verify-runner-source-manifest.mjs --manifest runner-source.json [--root .] [--phase before-build|after-build]');
  }
  console.log(JSON.stringify(await verifyRunnerSourceManifest({ root, manifestPath: path.resolve(root, manifest), phase }), null, 2));
}

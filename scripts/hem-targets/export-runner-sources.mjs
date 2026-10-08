#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This runner source exporter is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
// Build a closed source export for the standalone Rust FHS runner.
// This copies bytes; it does not grant rights, create commits, or publish.
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const comparePath = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const HEM = '62d3df705690f33b3fc3e905c9971d4f3743bf2e';
const FHS = 'c5ba2673fbd886cfe4fb528f61b376bdf406ebbd';
const CORE = [
  'hem-target-core', 'hem_target_wrapper', 'hem_engine_upstream', 'hem_fhs_upstream',
  'community/crates/vulcan-model-transform', 'community/crates/vulcan-csv-codec',
  'vendor/jsonschema-0.46.5-offline',
];
const EXTRA_FILES = [
  '.cargo/config.toml',
  'community/data/schemas/core-input.schema.json', 'community/data/schemas/input_fhs.schema.json',
  'community/data/defaults/defaults_template.json',
  'community/ADDITIONAL_TERMS.md', 'community/ATTRIBUTION.md', 'community/LICENSE',
  'community/NOTICE', 'community/PATH_RIGHTS.md', 'community/TRADEMARKS.md',
  'community/licence-manifest.json', 'community/vulcan-origin.json',
];
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

function run(command, args, cwd) {
  return execFileSync(command, args, { cwd, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, CARGO_TERM_COLOR: 'never' } });
}

function buildHelper() {
  return `#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# This runner build helper is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
MANIFEST="$ROOT/runner-source.json"
OUT_FINAL="$(node -e 'const p=require("node:path"),f=require("node:fs"),x=p.resolve(process.argv[1]),parent=f.realpathSync(p.dirname(x));process.stdout.write(p.join(parent,p.basename(x)))' "\${1:?Give an output directory outside the source tree}")"
case "$OUT_FINAL/" in "$ROOT/"*) echo 'Output must be outside the source repository' >&2; exit 2;; esac
[ ! -e "$OUT_FINAL" ] || { echo "Output already exists: $OUT_FINAL" >&2; exit 2; }
node "$ROOT/scripts/hem-targets/verify-runner-source-manifest.mjs" --manifest "$MANIFEST" --root "$ROOT" --phase before-build
OUT="$(mktemp -d "\${TMPDIR:-/tmp}/hem-target-wasm.XXXXXX")"
trap 'rm -rf "$OUT"' EXIT
export RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+simd128,+nontrapping-fptoint,+sign-ext,+extended-const --cfg getrandom_backend="wasm_js"'
export VULCAN_WASM_BUILD_ID="runner-$(node -e 'const m=require(process.argv[1]);process.stdout.write(m.exportFilesSha256.slice(0,16))' "$MANIFEST")"
export VULCAN_SOURCE_GIT_COMMIT="$(node -e 'const m=require(process.argv[1]);process.stdout.write(m.sourceCommit)' "$MANIFEST")"
export VULCAN_SOURCE_TREE_SHA256="$(node -e 'const m=require(process.argv[1]);process.stdout.write(m.sourceTreeSha256)' "$MANIFEST")"
export VULCAN_SOURCE_TREE_DIRTY="$(node -e 'const m=require(process.argv[1]);process.stdout.write(String(m.sourceTreeDirty))' "$MANIFEST")"
mkdir -p "$OUT/pkg"
cd "$ROOT/hem_target_wrapper"
wasm-pack build --target web --release --out-name wasm_wrapper --out-dir "$OUT/pkg" . -- --no-default-features -Z build-std=std,panic_abort
cp "$ROOT/hem_target_wrapper/browser_env_shim.js" "$OUT/pkg/browser_env_shim.js"
node --input-type=module - "$OUT/pkg/wasm_wrapper.js" <<'NODE'
import fs from 'node:fs';
const file = process.argv[2];
const js = fs.readFileSync(file, 'utf8')
  .replaceAll("from 'env'", "from './browser_env_shim.js'")
  .replaceAll('from "env"', 'from "./browser_env_shim.js"');
fs.writeFileSync(file, js);
NODE
node "$ROOT/scripts/hem-targets/patch-wasm-rayon-worker-helper.mjs" --root "$OUT/pkg"
node "$ROOT/scripts/hem-targets/patch-wasm-rayon-worker-helper.mjs" --check --root "$OUT/pkg"
cat "$ROOT/scripts/hem-targets/licensing/runtime-header.js" "$OUT/pkg/wasm_wrapper.js" > "$OUT/pkg/wasm_wrapper.js.with-header"
mv "$OUT/pkg/wasm_wrapper.js.with-header" "$OUT/pkg/wasm_wrapper.js"
for NOTICE_FILE in LICENSE ADDITIONAL_TERMS.md NOTICE.md ATTRIBUTION.md TRADEMARKS.md FHS-MIT-LICENSE.md; do
  cp "$ROOT/$NOTICE_FILE" "$OUT/pkg/$NOTICE_FILE"
done
node "$ROOT/scripts/hem-targets/verify-runner-source-manifest.mjs" --manifest "$MANIFEST" --root "$ROOT" --phase after-build
[ ! -e "$OUT_FINAL" ] || { echo "Output appeared during build: $OUT_FINAL" >&2; exit 2; }
cp -R "$OUT/pkg" "$OUT_FINAL"
echo "Raw FHS target WASM candidate: $OUT_FINAL"
echo 'This raw build omits source-checkout post-build hashing/manifest decoration and is not qualified or released.'
`;
}

async function collectTree(sourceRoot, relativeRoot, files) {
  async function visit(relative) {
    const absolute = path.join(sourceRoot, relative);
    const entries = await fs.readdir(absolute, { withFileTypes: true });
    entries.sort((a, b) => a.name.localeCompare(b.name, 'en'));
    for (const entry of entries) {
      if (entry.name === '.git' || entry.name === 'target') continue;
      const rel = `${relative}/${entry.name}`;
      const stat = await fs.lstat(path.join(sourceRoot, rel));
      if (stat.isSymbolicLink()) throw new Error(`Refusing source symlink: ${rel}`);
      if (stat.isDirectory()) await visit(rel);
      else if (stat.isFile()) files.set(rel, await fs.readFile(path.join(sourceRoot, rel)));
      else throw new Error(`Refusing non-regular source: ${rel}`);
    }
  }
  await visit(relativeRoot);
}

async function addExactFiles(sourceRoot, files) {
  for (const relative of EXTRA_FILES) {
    const stat = await fs.lstat(path.join(sourceRoot, relative));
    if (!stat.isFile() || stat.isSymbolicLink()) throw new Error(`Required source file is not regular: ${relative}`);
    let bytes = await fs.readFile(path.join(sourceRoot, relative));
    if (relative === '.cargo/config.toml') {
      const config = bytes.toString('utf8').replace('target-dir = ".cargo-target"', 'target-dir = "../rust-hem-runner/.cargo-target"');
      if (config === bytes.toString('utf8')) throw new Error('Expected shared Cargo target-dir declaration was not found');
      bytes = Buffer.from(config);
    }
    files.set(relative, bytes);
  }
}

async function validateUpstream(sourceRoot, name, expected) {
  const cwd = path.join(sourceRoot, name);
  const head = run('git', ['rev-parse', 'HEAD'], cwd).trim();
  if (head !== expected) throw new Error(`${name} is not at the approved immutable revision ${expected} (found ${head})`);
  const status = run('git', ['status', '--porcelain', '--untracked-files=all'], cwd).trim();
  if (status) throw new Error(`${name} is not pristine: ${status.split('\n')[0]}`);
  return head;
}

async function clearPreviousExport(destination) {
  const manifestPath = path.join(destination, 'runner-source.json');
  const priorBytes = await fs.readFile(manifestPath);
  const prior = JSON.parse(priorBytes.toString('utf8'));
  if (!Array.isArray(prior.files)) throw new Error('Existing destination has an invalid runner-source.json; refusing to replace it');
  const expected = new Map();
  for (const item of prior.files) {
    if (!item || typeof item.path !== 'string' || item.path.startsWith('/') || item.path.split('/').some(x => !x || x === '.' || x === '..') || !/^[a-f0-9]{64}$/.test(item.sha256 ?? '') || expected.has(item.path)) {
      throw new Error('Existing destination manifest has an unsafe or duplicate path; refusing to replace it');
    }
    expected.set(item.path, item.sha256);
  }
  const actual = new Set();
  async function walk(directory, prefix = '') {
    for (const entry of await fs.readdir(directory, { withFileTypes: true })) {
      if (!prefix && entry.name === '.git') continue;
      if (!prefix && entry.name === 'runner-source.json') continue;
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      const absolute = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`Unexpected symlink in existing export: ${relative}`);
      if (entry.isDirectory()) await walk(absolute, relative);
      else if (entry.isFile()) {
        const digest = sha(await fs.readFile(absolute));
        if (expected.get(relative) !== digest) throw new Error(`Existing export contains an unlisted or changed file: ${relative}`);
        actual.add(relative);
      } else throw new Error(`Unexpected special file in existing export: ${relative}`);
    }
  }
  await walk(destination);
  if (actual.size !== expected.size) throw new Error('Existing export differs from its source manifest; refusing to replace it');
  for (const relative of actual) await fs.unlink(path.join(destination, relative));
  await fs.unlink(manifestPath);
  const directories = [];
  async function collect(directory) {
    for (const entry of await fs.readdir(directory, { withFileTypes: true })) {
      if (directory === destination && entry.name === '.git') continue;
      if (entry.isDirectory()) { const child = path.join(directory, entry.name); await collect(child); directories.push(child); }
    }
  }
  await collect(destination);
  for (const directory of directories.sort((a, b) => b.length - a.length)) await fs.rmdir(directory);
  return prior.previousCandidateIdentity ?? {
    manifestSha256: sha(priorBytes), sourceCommit: prior.sourceCommit,
    sourceTreeSha256: prior.sourceTreeSha256, exportFilesSha256: prior.exportFilesSha256,
    sourceTreeDirty: prior.sourceTreeDirty, originGitTree: prior.originGitTree,
  };
}

async function localCargoRoots(sourceRoot) {
  const raw = run('cargo', ['metadata', '--offline', '--locked', '--manifest-path', 'hem_target_wrapper/Cargo.toml', '--format-version', '1'], sourceRoot);
  const metadata = JSON.parse(raw);
  const actual = new Set(metadata.packages.filter(pkg => pkg.source === null)
    .map(pkg => path.relative(sourceRoot, path.dirname(pkg.manifest_path)).split(path.sep).join('/')));
  const expected = new Set(CORE);
  const unexpected = [...actual].filter(dir => !expected.has(dir));
  const absent = [...expected].filter(dir => !actual.has(dir));
  if (unexpected.length || absent.length) throw new Error(`Local Cargo dependency closure mismatch; unexpected=[${unexpected.join(', ')}], missing=[${absent.join(', ')}]`);
  return [...actual].sort();
}

function defaultDocs() {
  return `# Vulcan HEM FHS runners\n\nCorresponding Source: [source tree at hem-fhs-preparation-2026-10-08](https://github.com/vulcan-energy/vulcan-hem-runners/tree/hem-fhs-preparation-2026-10-08). The complete source and build-material archives are linked from the [matching GitHub release](https://github.com/vulcan-energy/vulcan-hem-runners/releases/tag/hem-fhs-preparation-2026-10-08).\n\nThis repository contains the narrow FHS target runner source closure for that release.\n\n## Scope\n\nThe release contains the shared \`hem-target-core\` and thin \`hem_target_wrapper\`, the \`hem-target-runtime\` TypeScript host and workers that prepare models and drive the runners in a browser or CLI, pristine pinned HEM and FHS upstream trees, the Community \`vulcan-model-transform\` and \`vulcan-csv-codec\` crates with their schema/default inputs and notices, and the offline-patched \`jsonschema\` dependency. It excludes the private Vulcan SAP/PV integration in \`hem-batch-core\`, \`wasm_wrapper\` Rust sources, and generic Free editor code; \`hem-target-runtime\` imports the Community geometry-editor packages and the \`@repo/core\` scenario helpers by name, and those are not part of this closure. The pinned upstream model sources remain included as runner dependencies.\n\nThe newly extracted first-party core, wrapper, TypeScript runtime and runner build helpers are licensed under AGPL-3.0-only, supplemented by Vulcan Origin Terms v1.0. The owner approved this scope on 2026-10-06. See \`LICENSE\`, \`ADDITIONAL_TERMS.md\`, \`NOTICE.md\`, \`ATTRIBUTION.md\` and \`TRADEMARKS.md\`. The unchanged proprietary root licence from the source repository is not included. The first-party licence grant is separate from the Community transform's own AGPL-3.0-only and Origin Terms.\n\nThe Community, upstream and vendored notices remain with their respective source trees. The approved FHS MIT authority remains the pinned Community decision for \`c5ba2673fbd886cfe4fb528f61b376bdf406ebbd\`, with canonical evidence revision \`dd5ba73a19674d631da59b4924bb7dc2833fbb3b\`.\n\n## Source identity and build\n\n\`runner-source.json\` binds the originating source commit to the exact exported file inventory hash; it records each exported file SHA-256 and pinned upstream revisions. The export hash is the build source-tree identity; the original root Git tree and prior candidate identity are retained as origin metadata. Verify the exact inventory before and after a build:\n\n\`node scripts/hem-targets/verify-runner-source-manifest.mjs --manifest runner-source.json --root . --phase before-build\`\n\nUse \`scripts/build-fhs-target-wasm.sh /path/outside/this/repository\` to reproduce the standalone FHS WASM build with the same target-feature flags and wasm-pack build-std settings as the source checkout's \`fhs-target\` lane. Cargo uses the neighboring \`rust-hem-runner/.cargo-target\` cache; generated package output and temporary state stay outside this repository. The helper uses the runner-scoped Rayon bridge and applies \`scripts/hem-targets/licensing/runtime-header.js\` to generated glue. It emits a local raw artifact; the root build script's hash injection and final artifact decoration are omitted. A successful raw build is not a qualification, registration, or release decision.\n\nThe exporter and verifier live under \`scripts/hem-targets/\` in the source checkout. Exporting writes local files only; it does not create commits, register artifacts, or publish them.\n`;
}

export async function exportRunnerSources({ sourceRoot = ROOT, destination }) {
  if (!destination) throw new Error('Destination is required');
  sourceRoot = path.resolve(sourceRoot); destination = path.resolve(destination);
  const destEntries = await fs.readdir(destination);
  let previousCandidateIdentity = null;
  if (destEntries.some(entry => entry !== '.git')) {
    if (!destEntries.includes('runner-source.json')) throw new Error(`Refusing non-empty destination: ${destination}`);
    previousCandidateIdentity = await clearPreviousExport(destination);
  }
  for (const name of ['hem_engine_upstream', 'hem_fhs_upstream']) {
    if (!await fs.stat(path.join(sourceRoot, name)).catch(() => null)) throw new Error(`Missing upstream tree: ${name}`);
  }
  const upstreamRevisions = {
    hemEngine: await validateUpstream(sourceRoot, 'hem_engine_upstream', HEM),
    hemFhs: await validateUpstream(sourceRoot, 'hem_fhs_upstream', FHS),
    community: {
      head: run('git', ['rev-parse', 'HEAD'], path.join(sourceRoot, 'community')).trim(),
      workingTreeDirty: Boolean(run('git', ['status', '--porcelain', '--untracked-files=all'], path.join(sourceRoot, 'community')).trim()),
      trackedDiffSha256: sha(run('git', ['diff', '--binary', 'HEAD'], path.join(sourceRoot, 'community'))),
    },
  };
  const localRoots = await localCargoRoots(sourceRoot);
  const files = new Map();
  for (const root of CORE) await collectTree(sourceRoot, root, files);
  // The TypeScript host/worker layer that drives these runners; not a Cargo root.
  await collectTree(sourceRoot, 'hem-target-runtime', files);
  await collectTree(sourceRoot, 'community/LICENSES', files);
  await collectTree(sourceRoot, 'scripts/hem-targets/licensing', files);
  await addExactFiles(sourceRoot, files);
  for (const name of ['LICENSE', 'ADDITIONAL_TERMS.md', 'NOTICE.md', 'ATTRIBUTION.md', 'TRADEMARKS.md']) {
    files.set(name, await fs.readFile(path.join(sourceRoot, 'scripts/hem-targets/licensing', name)));
  }
  files.set('FHS-MIT-LICENSE.md', await fs.readFile(path.join(sourceRoot, 'scripts/hem-targets/licensing/third-party/FHS-MIT.md')));
  // Include the closed export machinery, its focused tests, and runner-only build helper.
  for (const relative of [
    'scripts/hem-targets/export-runner-sources.mjs',
    'scripts/hem-targets/verify-runner-source-manifest.mjs',
    'scripts/hem-targets/__tests__/export-runner-sources.test.mjs',
  ]) files.set(relative, await fs.readFile(path.join(sourceRoot, relative)));
  const genericRayonHelperPath = 'scripts/patch-wasm-rayon-worker-helper.mjs';
  const genericRayonHelper = await fs.readFile(path.join(sourceRoot, genericRayonHelperPath));
  const shebangEnd = genericRayonHelper.indexOf(0x0a) + 1;
  if (shebangEnd < 1) throw new Error('Runner Rayon bridge source has no node shebang');
  const scopedHeader = Buffer.from('// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors\n// SPDX-License-Identifier: AGPL-3.0-only\n// Approved runner build helper copy; subject to Vulcan Origin Terms v1.0. See scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.\n');
  files.set('scripts/hem-targets/patch-wasm-rayon-worker-helper.mjs', Buffer.concat([
    genericRayonHelper.subarray(0, shebangEnd), scopedHeader, genericRayonHelper.subarray(shebangEnd),
  ]));
  files.set('scripts/build-fhs-target-wasm.sh', Buffer.from(buildHelper()));
  files.set('README.md', Buffer.from(defaultDocs()
    .replaceAll('`licensing/runtime-header.js`', '`scripts/hem-targets/licensing/runtime-header.js`')
    .replaceAll('and `TRADEMARKS.md`.', '`TRADEMARKS.md`, and `FHS-MIT-LICENSE.md`.')));
  const sourceCommit = run('git', ['rev-parse', 'HEAD'], sourceRoot).trim();
  const originGitTree = run('git', ['show', '-s', '--format=%T', 'HEAD'], sourceRoot).trim();
  if (!/^[a-f0-9]{40}$/.test(sourceCommit) || !/^[a-f0-9]{40}$/.test(originGitTree)) throw new Error('Could not resolve source checkout identity');
  const status = run('git', ['status', '--porcelain', '--untracked-files=all'], sourceRoot).trim();
  const entries = [...files].map(([filePath, bytes]) => ({ path: filePath, sha256: sha(bytes) }))
    .sort((a, b) => comparePath(a.path, b.path));
  const exportFilesSha256 = sha(entries.map(item => `${item.path}\0${item.sha256}\n`).join(''));
  const manifest = { schemaVersion: 1, sourceCommit, sourceTreeSha256: exportFilesSha256,
    sourceTreeDirty: true, originGitTree, originWorkingTreeDirty: Boolean(status),
    exportFilesSha256, files: entries, upstreamRevisions, localCargoRoots: localRoots, previousCandidateIdentity,
    derivedFiles: [{ path: 'scripts/hem-targets/patch-wasm-rayon-worker-helper.mjs',
      sourcePath: genericRayonHelperPath, sourceSha256: sha(genericRayonHelper),
      transformation: 'Preserved original file bytes and shebang; added the runner AGPL-3.0-only SPDX and Origin Terms header only.' }] };
  const temp = await fs.mkdtemp(path.join(os.tmpdir(), 'hem-runner-export-'));
  try {
    for (const [relative, bytes] of files) {
      const target = path.join(temp, relative);
      await fs.mkdir(path.dirname(target), { recursive: true });
      await fs.writeFile(target, bytes, { flag: 'wx' });
      await fs.chmod(target, relative === 'scripts/build-fhs-target-wasm.sh' ? 0o755 : 0o644);
    }
    await fs.writeFile(path.join(temp, 'runner-source.json'), `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx' });
    // Copy only after every source and dependency check has succeeded.
    for (const relative of [...files.keys()].sort()) {
      const target = path.join(destination, relative);
      await fs.mkdir(path.dirname(target), { recursive: true });
      await fs.copyFile(path.join(temp, relative), target, fs.constants.COPYFILE_EXCL);
    }
    await fs.copyFile(path.join(temp, 'runner-source.json'), path.join(destination, 'runner-source.json'), fs.constants.COPYFILE_EXCL);
  } catch (error) {
    for (const topLevel of new Set([...files.keys(), 'runner-source.json'].map(relative => relative.split('/')[0]))) {
      await fs.rm(path.join(destination, topLevel), { recursive: true, force: true }).catch(() => {});
    }
    throw error;
  } finally { await fs.rm(temp, { recursive: true, force: true }); }
  return { destination, files: entries.length, exportFilesSha256, sourceCommit, sourceTreeSha256: exportFilesSha256,
    sourceTreeDirty: manifest.sourceTreeDirty, originGitTree, originWorkingTreeDirty: manifest.originWorkingTreeDirty,
    upstreamRevisions, localCargoRoots: localRoots,
    public: false, committed: false };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const [destination, ...rest] = process.argv.slice(2);
  if (!destination || rest.length) throw new Error('Usage: node scripts/hem-targets/export-runner-sources.mjs <empty-private-repository-directory>');
  console.log(JSON.stringify(await exportRunnerSources({ destination }), null, 2));
}

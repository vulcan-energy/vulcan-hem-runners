// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This runner source test is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { verifyRunnerSourceManifest } from '../verify-runner-source-manifest.mjs';

const sha = value => createHash('sha256').update(value).digest('hex');
const inventoryHash = entries => sha(entries.map(item => `${item.path}\0${item.sha256}\n`).join(''));

async function fixture(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'runner-export-test-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  await fs.mkdir(path.join(root, '.git'));
  const data = Buffer.from('source bytes\n');
  await fs.writeFile(path.join(root, 'src.rs'), data);
  const files = [{ path: 'src.rs', sha256: sha(data) }];
  const manifest = { schemaVersion: 1, sourceCommit: 'a'.repeat(40), sourceTreeSha256: inventoryHash(files),
    sourceTreeDirty: true, exportFilesSha256: inventoryHash(files), files };
  const manifestPath = path.join(root, 'runner-source.json');
  await fs.writeFile(manifestPath, JSON.stringify(manifest));
  return { root, manifest, manifestPath, data };
}

test('runner source verifier accepts exact listed files and ignores only repository metadata', async t => {
  const f = await fixture(t);
  const result = await verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath });
  assert.equal(result.files, 1);
});

test('runner source verifier rejects changed, missing, and newly added source files', async t => {
  const f = await fixture(t);
  await fs.writeFile(path.join(f.root, 'src.rs'), 'changed\n');
  await assert.rejects(verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath }), /Runner source changed/);
  await fs.writeFile(path.join(f.root, 'src.rs'), f.data);
  await fs.writeFile(path.join(f.root, 'extra.rs'), 'unlisted\n');
  await assert.rejects(verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath }), /Runner source changed/);
  await fs.rm(path.join(f.root, 'src.rs'));
  await assert.rejects(verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath }), /Runner source changed/);
});

test('runner source verifier rejects path traversal and inventory digest tampering', async t => {
  const f = await fixture(t);
  f.manifest.files[0].path = '../outside';
  f.manifest.exportFilesSha256 = inventoryHash(f.manifest.files);
  await fs.writeFile(f.manifestPath, JSON.stringify(f.manifest));
  await assert.rejects(verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath }), /Invalid runner source manifest file entry/);
  f.manifest.files[0].path = 'src.rs';
  f.manifest.exportFilesSha256 = '0'.repeat(64);
  await fs.writeFile(f.manifestPath, JSON.stringify(f.manifest));
  await assert.rejects(verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath }), /inventory digest mismatch/);
  f.manifest.exportFilesSha256 = inventoryHash(f.manifest.files);
  f.manifest.sourceTreeSha256 = '1'.repeat(64);
  await fs.writeFile(f.manifestPath, JSON.stringify(f.manifest));
  await assert.rejects(verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath }), /tree identity does not match/);
});

test('runner source verifier inventories nested files named runner-source.json', async t => {
  const f = await fixture(t);
  await fs.mkdir(path.join(f.root, 'nested'));
  await fs.writeFile(path.join(f.root, 'nested/runner-source.json'), 'not the root manifest');
  await assert.rejects(verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath }), /Runner source changed/);
});

test('runner source verifier rejects symlink files rather than hashing their targets', async t => {
  const f = await fixture(t);
  await fs.symlink(path.join(f.root, 'src.rs'), path.join(f.root, 'alias.rs'));
  await assert.rejects(verifyRunnerSourceManifest({ root: f.root, manifestPath: f.manifestPath }), /Symlink in runner source tree/);
});

test('runner extracted source declares the owner-approved licence and Origin Terms', async () => {
  const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../..');
  const read = relative => fs.readFile(path.join(root, relative), 'utf8');
  const [coreManifest, wrapperManifest, rootLicense, runnerLicense, communityLicense, originTerms, communityTerms,
    runnerTrademarks, communityTrademarks, notice] = await Promise.all([
    read('hem-target-core/Cargo.toml'), read('hem_target_wrapper/Cargo.toml'), read('LICENSE'),
    read('scripts/hem-targets/licensing/LICENSE'), read('community/LICENSE'),
    read('scripts/hem-targets/licensing/ADDITIONAL_TERMS.md'), read('community/ADDITIONAL_TERMS.md'),
    read('scripts/hem-targets/licensing/TRADEMARKS.md'), read('community/TRADEMARKS.md'),
    read('scripts/hem-targets/licensing/NOTICE.md'),
  ]);
  assert.match(coreManifest, /^license = "AGPL-3\.0-only"$/m);
  assert.match(wrapperManifest, /^license = "AGPL-3\.0-only"$/m);
  assert.doesNotMatch(`${coreManifest}\n${wrapperManifest}`, /license-file/);
  assert.match(runnerLicense, /GNU AFFERO GENERAL PUBLIC LICENSE/);
  assert.equal(runnerLicense, communityLicense);
  try {
    await fs.access(path.join(root, 'ADDITIONAL_TERMS.md'));
    assert.equal(rootLicense, runnerLicense, 'export-root LICENSE must match the AGPL runner licence');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    assert.match(rootLicense, /^PROPRIETARY SOFTWARE LICENSE/, 'the implementation checkout root licence stays unchanged');
  }
  assert.equal(originTerms, communityTerms);
  assert.equal(runnerTrademarks, communityTrademarks);
  assert.match(notice, /owner approved these terms on 2026-10-06/);
  const attribution = await read('scripts/hem-targets/licensing/ATTRIBUTION.md');
  const releaseUrl = 'https://github.com/vulcan-energy/vulcan-hem-runners/tree/hem-fhs-mvp-2026-10-07';
  assert.match(notice, new RegExp(releaseUrl.replaceAll('.', '\\.'), 'u'));
  assert.match(attribution, new RegExp(releaseUrl.replaceAll('.', '\\.'), 'u'));
  const releaseAssetsUrl = 'https://github.com/vulcan-energy/vulcan-hem-runners/releases/tag/hem-fhs-mvp-2026-10-07';
  assert.match(notice, new RegExp(releaseAssetsUrl.replaceAll('.', '\\.'), 'u'));
  assert.match(attribution, new RegExp(releaseAssetsUrl.replaceAll('.', '\\.'), 'u'));
  assert.doesNotMatch(`${notice}\n${attribution}`, /private and has not been released|private-candidate|before any release, replace/i);
  const fhsMit = await read('scripts/hem-targets/licensing/third-party/FHS-MIT.md');
  assert.equal(sha(fhsMit), 'af4b205d8259dd875442e4473dbd0d8ef638315d5dbb00a3d7d5fb1489f13cae');
  assert.match(notice, /FHS-MIT-LICENSE\.md/);
  if (await fs.stat(path.join(root, 'FHS-MIT-LICENSE.md')).catch(() => null)) {
    assert.equal(await read('FHS-MIT-LICENSE.md'), fhsMit);
  }
  for (const relative of [
    'hem-target-core/src/lib.rs', 'hem-target-core/src/runtime.rs',
    'hem_target_wrapper/src/lib.rs', 'hem_target_wrapper/build.rs',
    'scripts/hem-targets/export-runner-sources.mjs',
    'scripts/hem-targets/verify-runner-source-manifest.mjs',
    'scripts/hem-targets/__tests__/export-runner-sources.test.mjs',
    'scripts/hem-targets/licensing/runtime-header.js', 'hem_target_wrapper/browser_env_shim.js',
  ]) {
    const source = await read(relative);
    assert.match(source, /SPDX-License-Identifier: AGPL-3\.0-only/, relative);
    assert.match(source, /Vulcan Origin Terms v1\.0/, relative);
  }
  const exporter = await read('scripts/hem-targets/export-runner-sources.mjs');
  assert.ok(exporter.includes('releases/tag/hem-fhs-mvp-2026-10-07'), 'generated README links the full release source/build archives');
  assert.doesNotMatch(exporter, /private source candidate|private, review-only export/);
  assert.match(exporter, /scripts\/hem-targets\/patch-wasm-rayon-worker-helper\.mjs/);
  assert.match(exporter, /Preserved original file bytes and shebang/);
  assert.match(exporter, /scripts\/hem-targets\/licensing/);
  assert.match(exporter, /files\.set\(name, await fs\.readFile/);
  assert.match(exporter, /for NOTICE_FILE in LICENSE ADDITIONAL_TERMS\.md NOTICE\.md ATTRIBUTION\.md TRADEMARKS\.md FHS-MIT-LICENSE\.md/);
  assert.match(exporter, /files\.set\('FHS-MIT-LICENSE\.md'/);
  const sharedHelperPath = path.join(root, 'scripts/patch-wasm-rayon-worker-helper.mjs');
  if (await fs.stat(sharedHelperPath).catch(() => null)) {
    assert.doesNotMatch(await fs.readFile(sharedHelperPath, 'utf8'), /SPDX-License-Identifier: AGPL-3\.0-only/);
  } else {
    assert.match(await read('scripts/hem-targets/patch-wasm-rayon-worker-helper.mjs'), /SPDX-License-Identifier: AGPL-3\.0-only/);
  }
});

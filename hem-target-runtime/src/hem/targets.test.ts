// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { webcrypto } from 'node:crypto';
import { HEM_TARGETS, targetPreparationContract, targetScenarioModes, assertRustTargetVersions, type HemTarget, modelArtifacts, resolveBatchTarget, resolveHemTarget, validateTargetManifest, type HemTargetManifest } from './targets';
import { RELEASE_MANIFEST_HASHES, TargetAssets, sha256 } from './targetAssets';
import { executePreparedJobs } from './preparedRun';

beforeEach(() => vi.stubGlobal('crypto', webcrypto));
const target = HEM_TARGETS[1];
const manifest = (): HemTargetManifest => ({ bundleId: target.id, protocol: 1, engineCommit: target.engineCommit, wrapperCommit: target.wrapperCommit,
  conversionProfile: target.conversionProfile, preparation: { javascript: 'prep.js', wasm: 'prep.wasm', schema: 'schema.json', defaults: 'defaults.json' },
  runtime: { javascript: 'python.mjs', index: 'runtime/', wheels: ['hem.whl'], requiredVersions: { 'hem-core': '1.0.0a9', 'hem-fhs-wrapper': '1.0.0a9' } }, modes: ['actual'], artifacts: ['prep.js','prep.wasm','schema.json','defaults.json','python.mjs','hem.whl','runtime/pyodide.asm.js','runtime/pyodide.asm.wasm','runtime/pyodide-lock.json','runtime/python_stdlib.zip'].map(path => ({ path, bytes: 1, sha256: 'a'.repeat(64) })) });
describe('immutable target resolution', () => {
  it('pins a release manifest for every selectable target', () => {
    expect(Object.keys(RELEASE_MANIFEST_HASHES).sort()).toEqual(HEM_TARGETS.map(target => target.id).sort());
    for (const digest of Object.values(RELEASE_MANIFEST_HASHES)) expect(digest).toMatch(/^[a-f0-9]{64}$/);
  });
  it('never substitutes an absent or mixed saved pin', () => {
    expect(() => resolveHemTarget('old-unavailable')).toThrow('unavailable');
    expect(() => resolveBatchTarget([undefined])).toThrow('Open and save each base model');
    expect(() => resolveBatchTarget(HEM_TARGETS.map(target => target.id))).toThrow('cannot mix');
    expect(() => resolveBatchTarget([target.id], HEM_TARGETS[0].id)).toThrow('Saved batch');
  });
  it('rejects mismatched engine, missing artifacts, and traversal', () => {
    expect(() => validateTargetManifest({ ...manifest(), engineCommit: 'wrong' }, target)).toThrow('identity');
    expect(() => validateTargetManifest({ ...manifest(), artifacts: [] }, target)).toThrow('missing');
    const invalid = manifest(); invalid.artifacts[0].path = '../elsewhere';
    expect(() => validateTargetManifest(invalid, target)).toThrow('Invalid artifact');
  });
});
describe('complete runtime identity', () => {
  it('accepts only complete pinned Python assets and matching distributions', () => {
    expect(() => validateTargetManifest(manifest(), target)).not.toThrow();
    for (const required of ['pyodide.asm.js', 'pyodide.asm.wasm', 'pyodide-lock.json', 'python_stdlib.zip']) {
      const missing = manifest(); missing.artifacts = missing.artifacts.filter(artifact => !artifact.path.endsWith(required));
      expect(() => validateTargetManifest(missing, target)).toThrow('missing pinned artifact');
    }
    const wrong = manifest(); wrong.runtime.requiredVersions!['hem-fhs-wrapper'] = '1.0.0a7';
    expect(() => validateTargetManifest(wrong, target)).toThrow('matching');
  });
  it('rejects path aliases, inconsistent content sizes, and repeated modes', () => {
    for (const path of ['a//b', 'a/./b', 'a/', '/a']) {
      const invalid = manifest(); invalid.artifacts[0].path = path;
      expect(() => validateTargetManifest(invalid, target)).toThrow('Invalid artifact');
    }
    const invalid = manifest(); invalid.artifacts[1].bytes = 2;
    expect(() => validateTargetManifest(invalid, target)).toThrow('Conflicting sizes');
    const duplicated = manifest(); duplicated.modes.push('actual');
    expect(() => validateTargetManifest(duplicated, target)).toThrow('duplicate');
  });
  it('never treats source archives as executable dependencies', () => {
    const invalid = manifest();
    invalid.preparation.javascript = 'source/prep.js';
    invalid.artifacts.push({ path: 'source/prep.js', bytes: 1, sha256: 'b'.repeat(64) });
    expect(() => validateTargetManifest(invalid, target)).toThrow('reserved source');
    const invalidIndex = manifest(); invalidIndex.runtime.index = 'source/runtime/';
    expect(() => validateTargetManifest(invalidIndex, target)).toThrow('reserved source');
  });
  it('never treats published notices as executable dependencies', () => {
    const bundle = manifest();
    bundle.artifacts.push({ path: 'notices/runner.js', bytes: 1, sha256: 'b'.repeat(64) });
    expect(() => validateTargetManifest({ ...bundle, runtime: { ...bundle.runtime, javascript: 'notices/runner.js' } }, target)).toThrow('reserved');
    expect(() => validateTargetManifest({ ...bundle, runtime: { ...bundle.runtime, index: 'notices/' } }, target)).toThrow('reserved');
  });
  it('requires Rust WASM and rejects unsupported per-mode capabilities', () => {
    const rust = HEM_TARGETS[0];
    const pinned: HemTargetManifest = { ...manifest(), bundleId: rust.id, engineCommit: rust.engineCommit, wrapperCommit: rust.wrapperCommit, conversionProfile: rust.conversionProfile, runtime: { javascript: 'prep.js', wasm: 'prep.wasm' } };
    expect(() => validateTargetManifest(pinned, rust)).not.toThrow();
    expect(() => validateTargetManifest({ ...pinned, runtime: { javascript: 'prep.js' } }, rust)).toThrow('runtime WASM');
    expect(() => validateTargetManifest({ ...pinned, modes: ['actual-fee'] }, rust)).toThrow('complete compliance');
  });
});
describe('verified shared artifact cache', () => {
  it('streams byte progress before the file completes and caches only verified content', async () => {
    const data = new TextEncoder().encode('streamed');
    const artifact = { path: 'large.wasm', bytes: data.length, sha256: await sha256(data) };
    let stream!: ReadableStreamDefaultController<Uint8Array>;
    const body = new ReadableStream<Uint8Array>({ start(controller) { stream = controller; } });
    const cache = { match: async () => undefined, put: vi.fn(async () => {}) } as unknown as Cache;
    const progress = vi.fn();
    const download = new TargetAssets(cache, vi.fn(async () => new Response(body))).download({ ...manifest(), artifacts: [artifact] }, progress);
    await vi.waitFor(() => expect(progress).toHaveBeenCalledWith(0, data.length));
    stream.enqueue(data.slice(0, 3));
    await vi.waitFor(() => expect(progress).toHaveBeenCalledWith(3, data.length));
    expect(cache.put).not.toHaveBeenCalled();
    stream.enqueue(data.slice(3)); stream.close();
    await download;
    expect(progress).toHaveBeenLastCalledWith(data.length, data.length);
    expect(cache.put).toHaveBeenCalledOnce();
  });
  it('limits downloads to four requests and waits for their verified cache writes', async () => {
    const data = Array.from({ length: 6 }, (_, i) => new TextEncoder().encode(`file-${i}`));
    const artifacts = await Promise.all(data.map(async (bytes, i) => ({ path: `file-${i}`, bytes: bytes.length, sha256: await sha256(bytes) })));
    const finish: Array<() => void> = [];
    const fetcher = vi.fn((url: string | URL | Request) => new Promise<Response>(resolve => {
      const index = Number(String(url).split('-').pop());
      finish.push(() => resolve(new Response(data[index])));
    })) as unknown as typeof fetch;
    const cache = { match: async () => undefined, put: vi.fn(async () => {}) } as unknown as Cache;
    const download = new TargetAssets(cache, fetcher).download({ ...manifest(), artifacts }, vi.fn());
    await vi.waitFor(() => expect(fetcher).toHaveBeenCalledTimes(4));
    finish[0]();
    await vi.waitFor(() => expect(fetcher).toHaveBeenCalledTimes(5));
    expect(cache.put).toHaveBeenCalledTimes(1);
    finish[1]();
    await vi.waitFor(() => expect(fetcher).toHaveBeenCalledTimes(6));
    finish.slice(2).forEach(resolve => resolve());
    await download;
    expect(cache.put).toHaveBeenCalledTimes(6);
  });
  it('cancels while cache inspection is pending without starting network requests', async () => {
    const controller = new AbortController();
    let finish!: () => void;
    const cache = { match: () => new Promise<undefined>(resolve => { finish = () => resolve(undefined); }), put: vi.fn() } as unknown as Cache;
    const fetcher = vi.fn();
    const download = new TargetAssets(cache, fetcher).download(manifest(), vi.fn(), controller.signal);
    const rejected = expect(download).rejects.toMatchObject({ name: 'AbortError' });
    await vi.waitFor(() => expect(finish).toBeDefined());
    controller.abort(); finish();
    await rejected;
    expect(fetcher).not.toHaveBeenCalled();
    expect(cache.put).not.toHaveBeenCalled();
  });
  it('cancels a pending streamed read without caching partial bytes or starting later files', async () => {
    const controller = new AbortController();
    const cancel = vi.fn();
    const artifact = { path: 'runtime.wasm', bytes: 8, sha256: 'a'.repeat(64) };
    const cache = { match: async () => undefined, put: vi.fn() } as unknown as Cache;
    const fetcher = vi.fn(async () => new Response(new ReadableStream<Uint8Array>({ cancel })));
    const download = new TargetAssets(cache, fetcher).download({ ...manifest(), artifacts: [artifact] }, vi.fn(), controller.signal);
    const rejected = expect(download).rejects.toMatchObject({ name: 'AbortError' });
    await vi.waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
    controller.abort();
    await rejected;
    expect(cancel).toHaveBeenCalledOnce();
    expect(cache.put).not.toHaveBeenCalled();
  });

  it('acquires only the requested schema, reuses verified bytes, and rejects corruption', async () => {
    const data = new TextEncoder().encode('{"type":"object"}');
    const artifact = { path: 'preparation/input_fhs.schema.json', bytes: data.length, sha256: await sha256(data) };
    const records = new Map<string, Response>();
    const cache = { match: async (key: string) => records.get(key)?.clone(), put: async (key: string, response: Response) => { records.set(key, response.clone()); } } as unknown as Cache;
    const fetcher = vi.fn(async () => new Response(data));
    const assets = new TargetAssets(cache, fetcher);
    expect(Array.from(await assets.ensureBytes('target-a9', artifact))).toEqual(Array.from(data));
    expect(fetcher).toHaveBeenCalledWith('/hem-targets/target-a9/preparation/input_fhs.schema.json', { cache: 'no-store' });
    expect(Array.from(await assets.ensureBytes('target-a9', artifact))).toEqual(Array.from(data));
    expect(fetcher).toHaveBeenCalledTimes(1);
    for (const key of records.keys()) records.set(key, new Response('broken'));
    fetcher.mockImplementation(async () => new Response('corrupt download'));
    await expect(assets.ensureBytes('target-a9', artifact)).rejects.toThrow('corrupt');
  });

  it('counts shared missing bytes once and detects corruption on reuse', async () => {
    const data = new TextEncoder().encode('pinned');
    const artifact = { path: 'a', bytes: data.length, sha256: await sha256(data) };
    const records = new Map<string, Response>();
    const cache = { match: async (key: string) => records.get(key)?.clone(), put: async (key: string, value: Response) => { records.set(key, value.clone()); }, delete: async (key: string) => records.delete(key) } as unknown as Cache;
    const fetcher = vi.fn(async () => new Response(data));
    const assets = new TargetAssets(cache, fetcher);
    const bundle = { ...manifest(), artifacts: [artifact, { ...artifact, path: 'shared' }] };
    expect((await assets.inspect(bundle)).missingBytes).toBe(6);
    await assets.download(bundle, () => {});
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect((await assets.inspect(bundle)).missingBytes).toBe(0);
    for (const key of records.keys()) records.set(key, new Response('broken'));
    expect((await assets.inspect(bundle)).missingBytes).toBe(6);
    expect(records.size).toBe(0);
  });
  it('calls the browser fetch with its global receiver', async () => {
    const data = new TextEncoder().encode('native-fetch');
    const artifact = { path: 'runtime.wasm', bytes: data.length, sha256: await sha256(data) };
    vi.stubGlobal('fetch', function(this: unknown) {
      if (this !== globalThis) throw new TypeError('Illegal invocation');
      return Promise.resolve(new Response(data));
    });
    try {
      const cache = { match: async () => undefined, put: vi.fn(async () => {}) } as unknown as Cache;
      await new TargetAssets(cache).download({ ...manifest(), artifacts: [artifact] }, () => {});
      expect(cache.put).toHaveBeenCalledOnce();
    } finally { vi.unstubAllGlobals(); }
  });
  it('keeps source archives and notices published but outside model download and worker memory', async () => {
    const data = new TextEncoder().encode('runtime');
    const runtime = { path: 'prep.wasm', bytes: data.length, sha256: await sha256(data) };
    const source = { path: 'source/sources.tar.gz', bytes: 115262341, sha256: 'c'.repeat(64) };
    const records = new Map<string, Response>();
    const cache = { match: async (key: string) => records.get(key)?.clone(), put: async (key: string, response: Response) => { records.set(key, response.clone()); } } as unknown as Cache;
    const fetcher = vi.fn(async () => new Response(data));
    const assets = new TargetAssets(cache, fetcher);
    const notice = { path: 'notices/LICENSE', bytes: 12000, sha256: 'd'.repeat(64) };
    const bundle = { ...manifest(), artifacts: [runtime, source, notice] };
    expect(modelArtifacts(bundle)).toEqual([runtime]);
    expect(bundle.artifacts).toContain(source);
    expect(bundle.artifacts).toContain(notice);
    expect((await assets.inspect(bundle)).missingBytes).toBe(data.length);
    await assets.download(bundle, () => {});
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect((await assets.inspect(bundle)).missingBytes).toBe(0);
  });
  it('rejects corrupted acquisition and missing offline assets', async () => {
    const cache = { match: async () => undefined, put: vi.fn() } as unknown as Cache;
    const assets = new TargetAssets(cache, vi.fn(async () => new Response('bad')));
    await expect(assets.download(manifest(), () => {})).rejects.toThrow('corrupt');
    expect(cache.put).not.toHaveBeenCalled();
    await expect(assets.bytes(manifest().artifacts[0])).rejects.toThrow('unavailable');
  });
});
describe('prepared execution lifecycle with stubbed calculation', () => {
  const job = { name: 'synthetic', input: '{}', modes: ['actual'], outputDirectory: 'output/synthetic', sourceCsv: 'synthetic CSV' };
  it('does not complete until the physical output write is acknowledged', async () => {
    let acknowledge!: () => void;
    const outputWritten = new Promise<void>(resolve => { acknowledge = resolve; });
    const statuses: string[] = [];
    const run = executePreparedJobs(manifest(), target.id, 'run-1', [job], async () => ({ 'small.csv': 'a\n1' }), async (path, content) => {
      if (path.endsWith('small.csv')) await outputWritten;
      if (path.endsWith('target-run.json')) statuses.push(JSON.parse(content).status);
    }, () => {});
    await vi.waitFor(() => expect(statuses).toEqual(['running']));
    acknowledge(); await run;
    expect(statuses).toEqual(['running','complete']);
  });
  it('failed writes cannot produce a success record', async () => {
    const statuses: string[] = [];
    await expect(executePreparedJobs(manifest(), target.id, 'run-1', [job], async () => ({ 'small.csv': 'a\n1' }), async (path, content) => {
      if (path.endsWith('small.csv')) throw new Error('disk full');
      if (path.endsWith('target-run.json')) statuses.push(JSON.parse(content).status);
    }, () => {})).rejects.toThrow('disk full');
    expect(statuses).toEqual(['running','failed']);
  });
  it('marks the archive failed if the visible running marker cannot be written', async () => {
    const archivedStatuses: string[] = [];
    const archivedJob = { ...job, archiveDirectory: 'output/synthetic/archive/run-1' };
    await expect(executePreparedJobs(manifest(), target.id, 'run-1', [archivedJob], async () => ({ 'small.csv': 'a\n1' }), async (path, content) => {
      const status = JSON.parse(content).status;
      if (path === `${job.outputDirectory}/target-run.json` && status === 'running') throw new Error('visible marker write failed');
      if (path === `${archivedJob.archiveDirectory}/target-run.json`) archivedStatuses.push(status);
    }, () => {})).rejects.toThrow('visible marker write failed');
    expect(archivedStatuses).toEqual(['running', 'failed']);
  });
  it('verifies and archives custom weather before executing all modes with the same bytes', async () => {
    const epw = 'small synthetic weather\r\n';
    const weather = { path: 'authored.epw', epw, sha256: await sha256(new TextEncoder().encode(epw)) };
    const files: Record<string, string> = {};
    const execute = vi.fn(async () => ({ 'small.csv': 'a\n1' }));
    const write = async (path: string, content: string) => { files[path] = content; };
    await executePreparedJobs(manifest(), target.id, 'run', [{ ...job, weather }], execute, write, vi.fn());
    expect(execute).toHaveBeenCalledWith(job.input, job.modes, weather);
    expect(files['output/synthetic/weather.epw']).toBe(epw);
    expect(JSON.parse(files['output/synthetic/target-run.json']).weather).toEqual({ path: weather.path, sha256: weather.sha256 });
    execute.mockClear();
    await expect(executePreparedJobs(manifest(), target.id, 'run', [{ ...job, weather: { ...weather, epw: 'tampered' } }], execute, write, vi.fn())).rejects.toThrow('weather hash');
    expect(execute).not.toHaveBeenCalled();
  });
  it('rejects mismatched targets and unsupported modes before execution', async () => {
    const execute = vi.fn();
    await expect(executePreparedJobs(manifest(), 'wrong', 'run', [job], execute, vi.fn(), vi.fn())).rejects.toThrow('identity');
    await expect(executePreparedJobs(manifest(), target.id, 'run', [{...job, modes: ['epc']}], execute, vi.fn(), vi.fn())).rejects.toThrow('mode');
    expect(execute).not.toHaveBeenCalled();
  });
});

describe('runtime-independent input contracts', () => {
  it('preserves every published preparation request identifier', () => {
    expect(HEM_TARGETS.map(target => targetPreparationContract(target))).toEqual([
      { profile: 'fhs', conversion_profile: 'current_rust_fhs', version_metadata: { hem_core_version: '1.0.0a7', fhs_wrapper_version: '1.0.0a7' } },
      { profile: 'fhs', conversion_profile: 'python_fhs_a9', version_metadata: { hem_core_version: '1.0.0a9', fhs_wrapper_version: '1.0.0a9' } },
      { profile: 'fhs', conversion_profile: 'python_fhs_a8', version_metadata: { hem_core_version: '1.0.0a8', fhs_wrapper_version: '1.0.0a8' } },
    ]);
  });
  it('shares one semantic preparation contract across Rust and Python with independent versions', () => {
    const future: HemTarget = { ...target, id: 'test-only-future', engineVersion: '2.0', wrapperVersion: '3.0', conversionProfile: 'physical_opening_full_partition_party_wall_u_v1' };
    const rust: HemTarget = { ...future, runtime: 'rust' };
    expect(targetPreparationContract(rust)).toEqual(targetPreparationContract(future));
    expect(targetPreparationContract(rust).version_metadata).toEqual({ hem_core_version: '2.0', fhs_wrapper_version: '3.0' });
    expect(() => assertRustTargetVersions(rust, '2.0', '3.0')).not.toThrow();
    expect(() => assertRustTargetVersions(rust, '2.0', '2.0')).toThrow('version mismatch');
    expect(() => assertRustTargetVersions(rust, undefined, '3.0')).toThrow('version mismatch');
    const bundle = { ...manifest(), bundleId: future.id, conversionProfile: future.conversionProfile, runtime: { ...manifest().runtime, requiredVersions: { 'hem-core': '2.0', 'hem-fhs-wrapper': '3.0' } } };
    expect(() => validateTargetManifest(bundle, future)).not.toThrow();
    expect(() => validateTargetManifest({ ...bundle, runtime: { ...bundle.runtime, requiredVersions: { 'hem-core': '2.0', 'hem-fhs-wrapper': '2.0' } } }, future)).toThrow('matching');
  });
  it('selects wrapper modes centrally and rejects unsupported wrappers', () => {
    expect(targetScenarioModes(target, 'fhs_assumptions')).toEqual(['actual']);
    expect(targetScenarioModes(target, 'fhs_compliance')).toEqual(['actual', 'actual-fee', 'notional', 'notional-fee']);
    expect(() => targetScenarioModes(target, 'epc')).toThrow('incompatible');
  });
});

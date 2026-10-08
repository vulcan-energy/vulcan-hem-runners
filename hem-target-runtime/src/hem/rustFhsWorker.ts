// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { TargetAssets } from './targetAssets';
import { resolveHemTarget, assertRustTargetVersions, validateTargetManifest, type HemTargetManifest } from './targets';
import { executePreparedJobs, type PreparedHemJob } from './preparedRun';
import { BatchWorkerFileSystemAcknowledgements } from '../workers/batchWorkerFileSystemBridge';
import { FS_REQUEST_TYPE, FS_RESPONSE_TYPE } from '../workers/workerProtocol';
let manifest: HemTargetManifest | undefined;
let runInput: ((input: string, output: string, modes: string, weather?: string) => string) | undefined;
let supportsWeather = false;
let busy = false;
const acknowledgements = new BatchWorkerFileSystemAcknowledgements();
const send = (message: unknown) => self.postMessage(message);
async function write(path: string, content: string) {
  const request = { type: FS_REQUEST_TYPE, operation: 'write_file' as const, path, content, messageId: crypto.randomUUID() };
  acknowledgements.register(request); send(request); await acknowledgements.waitForPending();
}
async function initialize(bundle: HemTargetManifest) {
  const target = resolveHemTarget(bundle.bundleId); validateTargetManifest(bundle, target);
  if (target.runtime !== 'rust' || target.wrapper !== 'fhs' || !bundle.runtime.wasm) throw new Error('Rust worker requires a pinned Rust runtime');
  const assets = await TargetAssets.open();
  const bytes = async (path: string) => {
    const artifact = bundle.artifacts.find(artifact => artifact.path === path);
    if (!artifact) throw new Error(`Missing Rust artifact ${path}`);
    return assets.bytes(artifact);
  };
  const url = URL.createObjectURL(new Blob([await bytes(bundle.runtime.javascript) as Uint8Array<ArrayBuffer>], { type: 'text/javascript' }));
  try {
    // Runtime code is loaded from the selected verified bundle, not the app bundle.
    // react-doctor-disable-next-line react-doctor/no-dynamic-import-path -- the pinned Rust module is imported from a blob URL.
    const module = await import(/* @vite-ignore */ url);
    await module.default({ module_or_path: await bytes(bundle.runtime.wasm) });
    assertRustTargetVersions(target, module.metadata_hem_core_version?.(), module.metadata_fhs_wrapper_version?.());
    if (typeof module.initialize_rayon_thread_pool !== 'function') throw new Error('Pinned Rust runtime lacks worker pool initialization');
    await module.initialize_rayon_thread_pool(1);
    supportsWeather = module.target_weather_protocol_version?.() === 1;
    if (typeof module.run_target_input !== 'function') throw new Error('Pinned runtime does not support prepared target jobs');
    runInput = module.run_target_input;
  } finally { URL.revokeObjectURL(url); }
  manifest = bundle;
  send({ type: 'worker_ready', targetBundleId: bundle.bundleId });
}
self.onmessage = async ({ data }) => {
  if (data.type === FS_RESPONSE_TYPE) { acknowledgements.acknowledge(data.messageId, data.error ?? null); return; }
  if (busy) { send({ type: 'error', jobId: data.jobId, error: 'A calculation is already active' }); return; }
  busy = true;
  try {
    if (data.type === 'initialize') await initialize(data.manifest);
    else if (data.type === 'run_prepared_batch') {
      if (!manifest || !runInput) throw new Error('Rust target is not initialized');
      await executePreparedJobs(manifest, data.targetBundleId, data.jobId, data.jobs as PreparedHemJob[], async (input, modes, weather) => {
        if (weather && !supportsWeather) throw new Error('Pinned Rust runtime does not implement custom EPW protocol 1; rebuild and qualify its bundle');
        const result = JSON.parse(weather ? runInput!(input, 'model', JSON.stringify(modes), weather.epw) : runInput!(input, 'model', JSON.stringify(modes)));
        if (result.status !== 'calculated') throw new Error(result.error ?? 'Rust calculation failed');
        return result.files;
      }, write, (completed, total) => send({ type: 'progress', jobId: data.jobId, progress: { completed, total } }));
      send({ type: 'complete', jobId: data.jobId, targetBundleId: manifest.bundleId, result: { status: 'success', targetBundleId: manifest.bundleId } });
    } else throw new Error('Unknown Rust target message');
  } catch (error) { send({ type: data.type === 'initialize' ? 'init_error' : 'error', jobId: data.jobId, error: String(error) }); }
  finally { busy = false; }
};

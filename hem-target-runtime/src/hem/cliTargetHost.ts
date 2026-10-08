// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { ecaasOnlyBatchMessage, loadTargetPreparation, snapshotBatchTarget, prepareTargetBatch, prepareTargetCsvWithManifest, type TargetBatchSnapshot } from './targetPreparation';
import { TargetAssets } from './targetAssets';
import { resolveHemTarget, validateTargetManifest, type HemTargetManifest } from './targets';

declare global {
  interface Window {
    targetCliRead(path: string): Promise<string>;
    targetCliWrite(path: string, content: string): Promise<void>;
    targetCliProgress(progress: unknown): Promise<void>;
  }
}

/** The CLI supplies verified local bytes; source preparation and calculation use the app modules. */
export async function run(request: {
  manifest: HemTargetManifest; operation: 'prepare' | 'snapshot' | 'batch'; csv?: string;
  config?: Record<string, unknown>; configName?: string; jobId: string;
}) {
  const { manifest } = request;
  const target = resolveHemTarget(manifest.bundleId);
  validateTargetManifest(manifest, target);
  if (request.operation === 'prepare') {
    if (typeof request.csv !== 'string') throw new Error('CSV source required');
    return prepareTargetCsvWithManifest(request.csv, manifest, window.targetCliRead);
  }
  const config = request.config;
  if (request.operation === 'snapshot') {
    if (!config) throw new Error('Batch configuration required');
    const snapshot = await snapshotBatchTarget(config, window.targetCliRead);
    if (snapshot.targetBundleId !== target.id) throw new Error('Batch source and supplied manifest target differ');
    return snapshot;
  }
  if (!config || !config.__hem) throw new Error('Save a target batch with its exact source/snippet snapshot before running it through MCP/CLI.');
  const ecaasOnly = ecaasOnlyBatchMessage(config, config.__hem as TargetBatchSnapshot);
  if (ecaasOnly) throw new Error(ecaasOnly);
  // Open -> download -> load is intentionally ordered: preparation reads only verified cached bytes.
  // react-doctor-disable-next-line react-doctor/async-parallel -- download depends on the opened cache; preparation depends on the completed download.
  const assets = await TargetAssets.open();
  await assets.download(manifest, (completed, total) => { void window.targetCliProgress({ status: 'running', stage: 'download', completed, total }); });
  const jobs = await prepareTargetBatch(config, config.__hem as TargetBatchSnapshot, manifest,
    request.configName!, request.jobId, await loadTargetPreparation(manifest));
  return new Promise((resolve, reject) => {
    const worker = new Worker(target.runtime === 'python' ? '/python-worker.mjs' : '/rust-worker.mjs', { type: 'module' });
    const finish = (error?: string) => {
      clearTimeout(startup);
      worker.terminate();
      if (error) reject(new Error(error));
      else resolve({ targetBundleId: target.id, status: 'complete', warnings: jobs.flatMap(job => (job.warnings ?? []).map(warning => `${job.name}: ${warning}`)) });
    };
    const startup = setTimeout(() => finish('Target worker startup exceeded 120 seconds'), 120_000);
    worker.onerror = event => finish(event.message || 'Target worker failed');
    worker.onmessageerror = () => finish('Target worker returned an unreadable message');
    worker.onmessage = async ({ data }) => {
      if (data.type === 'file_system') {
        try {
          if (data.operation !== 'write_file') throw new Error('Unexpected target filesystem operation');
          await window.targetCliWrite(data.path, data.content);
          worker.postMessage({ type: 'file_system_response', messageId: data.messageId, result: { success: true } });
        } catch (error) { worker.postMessage({ type: 'file_system_response', messageId: data.messageId, error: String(error) }); }
      } else if (data.type === 'worker_ready') {
        if (data.targetBundleId !== target.id) { finish('Worker target identity mismatch'); return; }
        clearTimeout(startup); // Python calculation duration is intentionally unbounded.
        worker.postMessage({ type: 'run_prepared_batch', targetBundleId: target.id, jobId: request.jobId, jobs });
      } else if (data.type === 'progress') {
        await window.targetCliProgress(data.progress);
      } else if (data.type === 'complete') {
        finish(data.targetBundleId === target.id ? undefined : 'Result target identity mismatch');
      } else if (data.type === 'error' || data.type === 'init_error') finish(data.error || 'Target worker reported an error');
    };
    worker.postMessage({ type: 'initialize', manifest });
  });
}

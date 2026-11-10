// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import type { PreparedHemJob } from './preparedRun';
import type { HemTargetManifest } from './targets';

export interface RustWorkerScope {
  location: { href: string };
  postMessage(message: unknown): void;
  onmessage: ((event: MessageEvent) => void) | null;
}

/** A fresh execution worker releases the entire WASM heap, including upstream Arc
 * cycles retained by PV diverters. Both browser and CLI load this same coordinator.
 * Terminating its child also terminates that child's Rayon workers. */
export function installIsolatedRustWorker(scope: RustWorkerScope) {
  let child: Worker | undefined;
  let manifest: HemTargetManifest | undefined;
  let batch: { targetBundleId: string; jobId: string; jobs: PreparedHemJob[] } | undefined;
  let completed = 0;
  let initialized = false;
  let starting = false;
  let startup: ReturnType<typeof setTimeout> | undefined;
  const fail = (error: string, jobId = batch?.jobId) => {
    clearTimeout(startup); child?.terminate(); child = undefined; starting = false;
    scope.postMessage({ type: initialized ? 'error' : 'init_error', jobId, error });
    batch = undefined;
  };
  const runJob = () => child!.postMessage({ type: 'run_prepared_batch', targetBundleId: batch!.targetBundleId, jobId: batch!.jobId, jobs: [batch!.jobs[completed]] });
  const start = () => {
    starting = true;
    const worker = new Worker(scope.location.href, { type: 'module', name: 'vulcan-rust-execution' });
    child = worker;
    startup = setTimeout(() => { if (child === worker) fail('Rust execution worker startup exceeded 120 seconds'); }, 120_000);
    worker.onerror = event => { if (child === worker) fail(event.message || 'Rust execution worker failed'); };
    worker.onmessageerror = () => { if (child === worker) fail('Rust execution worker returned an unreadable message'); };
    worker.onmessage = ({ data }) => {
      if (child !== worker) return;
      if (data.type === 'worker_ready') {
        if (data.targetBundleId !== manifest!.bundleId) { fail('Worker target identity mismatch'); return; }
        clearTimeout(startup); starting = false;
        if (!initialized) { initialized = true; scope.postMessage(data); }
        else if (batch) runJob();
      } else if (data.type === 'complete') {
        if (!batch || data.targetBundleId !== manifest!.bundleId || data.jobId !== batch.jobId) { fail('Completed worker identity mismatch'); return; }
        // The execution worker sends completion only after every filesystem acknowledgement.
        clearTimeout(startup); worker.terminate(); child = undefined; completed++;
        scope.postMessage({ type: 'progress', jobId: batch.jobId, progress: { completed, total: batch.jobs.length } });
        if (completed < batch.jobs.length) start();
        else { scope.postMessage(data); batch = undefined; }
      } else if (data.type === 'progress') {
        if (batch) scope.postMessage({ ...data, progress: { completed, total: batch.jobs.length } });
      } else if (data.type === 'error' || data.type === 'init_error') fail(data.error || 'Rust execution worker reported an error');
      else scope.postMessage(data);
    };
    worker.postMessage({ type: 'initialize', manifest });
  };
  scope.onmessage = ({ data }) => {
    if (data.type === 'file_system_response') { child?.postMessage(data); return; }
    if (batch || starting) { scope.postMessage({ type: 'error', jobId: data.jobId, error: 'A calculation or initialization is already active' }); return; }
    if (data.type === 'initialize') {
      clearTimeout(startup); child?.terminate(); manifest = data.manifest; initialized = false; start();
    } else if (data.type === 'run_prepared_batch') {
      if (!manifest || !initialized || data.targetBundleId !== manifest.bundleId || !data.jobs?.length) { fail('Rust target identity or prepared jobs are invalid', data.jobId); return; }
      batch = data; completed = 0;
      if (child) runJob(); else start();
    } else fail('Unknown Rust target message');
  };
}

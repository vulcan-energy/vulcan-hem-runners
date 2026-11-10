// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { sha256 } from './targetAssets';
import { parseHeatingProfile, type HeatingProfile } from './heatingProfile';
import type { HemTargetManifest } from './targets';
export interface PreparedWeather { path: string; epw: string; sha256: string; }
/** `parameterFiles` are the scenario's snippets, relative to batch_parameters; they name the scenario in a failure.
 * `startedAt` is when the run started (ISO 8601), the same for all its jobs; it orders runs whose files a copy re-dated. */
export interface PreparedHemJob { startedAt?: string; name: string; input: string; modes: string[]; outputDirectory: string; archiveDirectory?: string; sourceCsv?: string; weather?: PreparedWeather; warnings?: string[]; heatingProfile?: HeatingProfile; parameterFiles?: string[]; }
export async function executePreparedJobs(
  manifest: HemTargetManifest, bundleId: string, jobId: string, jobs: PreparedHemJob[],
  execute: (input: string, modes: string[], weather?: PreparedWeather, heatingProfile?: HeatingProfile) => Promise<Record<string, string>>,
  write: (path: string, content: string) => Promise<void>,
  progress: (completed: number, total: number) => void,
): Promise<void> {
  if (manifest.bundleId !== bundleId) throw new Error('Worker target identity mismatch');
  if (!jobs.length) throw new Error('Batch has no prepared jobs');
  const allowedModes = new Set(manifest.modes);
  const reservedNames = new Set(['model.json', 'source.csv', 'weather.epw', 'target-run.json']);
  for (const [index, job] of jobs.entries()) {
    if (!/^[a-zA-Z0-9_./ -]+$/.test(job.outputDirectory) || job.outputDirectory.split('/').some(segment => segment === '..') || !job.outputDirectory.startsWith('output/')) throw new Error('Invalid run output path');
    if (!job.modes.length || job.modes.some(mode => !allowedModes.has(mode))) throw new Error('Unsupported calculation mode for selected target');
    if (job.archiveDirectory && (!job.archiveDirectory.startsWith(`${job.outputDirectory}/`) || job.archiveDirectory.split('/').some(segment => segment === '..'))) throw new Error('Invalid archive path');
    if (job.heatingProfile) {
      parseHeatingProfile(job.heatingProfile);
      if (job.modes.length !== 1 || job.modes[0] !== 'actual') throw new Error('Heating profiles require actual alone; they are not FHS compliance calculations');
    }
    const archive = job.archiveDirectory ?? job.outputDirectory;
    // Jobs and all writes stay serialized: each marker/result write must be acknowledged before the next operation.
    // react-doctor-disable-next-line react-doctor/async-defer-await, react-doctor/async-await-in-loop -- the hash is included in this job's marker before any output is written.
    const inputSha256 = await sha256(new TextEncoder().encode(job.input));
    if (job.weather && (!job.weather.epw.trim() || await sha256(new TextEncoder().encode(job.weather.epw)) !== job.weather.sha256)) throw new Error('Prepared weather hash mismatch or empty EPW');
    const provenance = { ...(job.weather ? { weather: { path: job.weather.path, sha256: job.weather.sha256 } } : {}), ...(job.heatingProfile ? { calculationKind: 'operating-sensitivity', heatingProfile: job.heatingProfile } : {}), target: manifest, inputSha256, modes: job.modes, jobId, ...(job.startedAt ? { startedAt: job.startedAt } : {}), name: job.name, ...(job.warnings?.length ? { warnings: job.warnings } : {}) };
    try {
      // A crash or termination leaves this non-complete record. The original input is retained.
      await write(`${archive}/target-run.json`, JSON.stringify({ ...provenance, status: 'running' }));
      if (archive !== job.outputDirectory) await write(`${job.outputDirectory}/target-run.json`, JSON.stringify({ ...provenance, status: 'running', archive }));
      await write(`${archive}/model.json`, job.input);
      if (job.sourceCsv) await write(`${archive}/source.csv`, job.sourceCsv);
      if (job.weather) await write(`${archive}/weather.epw`, job.weather.epw);
      progress(index, jobs.length);
      const files = await execute(job.input, job.modes, job.weather, job.heatingProfile);
      const names = Object.keys(files);
      if (!names.length) throw new Error('Calculation produced no result files');
      for (const [name, content] of Object.entries(files)) {
        if (reservedNames.has(name) || !name || name === '.' || name === '..' || /[\\/]/.test(name)) throw new Error('Unexpected result path');
        // The filesystem bridge acknowledges one write at a time; keep result order stable.
        // react-doctor-disable-next-line react-doctor/async-await-in-loop -- parallel writes could overtake terminal markers or race cancellation repair.
        await write(`${archive}/${name}`, content);
      }
      if (archive !== job.outputDirectory) {
        // Compatibility copies are also serialized through the ordered filesystem bridge.
        // react-doctor-disable-next-line react-doctor/async-await-in-loop -- keep the archive and visible result copies ordered.
        for (const [name, content] of Object.entries(files)) await write(`${job.outputDirectory}/${name}`, content);
        await write(`${job.outputDirectory}/target-run.json`, JSON.stringify({ ...provenance, status: 'complete', files: names, archive }));
      }
      await write(`${archive}/target-run.json`, JSON.stringify({ ...provenance, status: 'complete', files: names }));
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      await write(`${archive}/target-run.json`, JSON.stringify({ ...provenance, status: 'failed', error: message }));
      if (archive !== job.outputDirectory) await write(`${job.outputDirectory}/target-run.json`, JSON.stringify({ ...provenance, status: 'failed', error: message, archive }));
      const files = job.parameterFiles?.length ? ` (${job.parameterFiles.join(', ')})` : '';
      throw new Error(`Scenario ${job.name}${files}: ${message}`);
    }
  }
}

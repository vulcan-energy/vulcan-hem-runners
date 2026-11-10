// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { beforeEach, expect, it, vi } from 'vitest';
import { webcrypto } from 'node:crypto';
import { executePreparedJobs, type PreparedHemJob } from './preparedRun';
import type { HemTargetManifest } from './targets';

beforeEach(() => vi.stubGlobal('crypto', webcrypto));

it('names the failing scenario and its parameter files, and still stops at the first failure', async () => {
  const manifest = { bundleId: 'target', modes: ['actual'] } as unknown as HemTargetManifest;
  const job = (name: string, parameterFiles: string[]): PreparedHemJob => ({ name, input: '{}', modes: ['actual'], outputDirectory: `output/result_c_${name}`, parameterFiles });
  const jobs = [job('scenario_1_0', ['hot_water_source/good_a.json']), job('scenario_1_1', ['hot_water_source/ghost.json']), job('scenario_1_2', ['hot_water_source/good_b.json'])];
  const execute = vi.fn(async (input: string) => {
    if (execute.mock.calls.length === 2) throw new Error("Request was considered invalid due to error:\nImmersion heater references an undeclared energy supply 'ghost supply'.");
    return { 'FHS__results.csv': input };
  });
  const written = new Map<string, string>();
  await expect(executePreparedJobs(manifest, 'target', 'job', jobs, execute, async (path, content) => { written.set(path, content); }, () => {})).rejects.toThrow(
    "Scenario scenario_1_1 (hot_water_source/ghost.json): Request was considered invalid due to error:\nImmersion heater references an undeclared energy supply 'ghost supply'.");
  expect(execute).toHaveBeenCalledTimes(2);
  // The run record keeps the engine's message, not a stringified "Error: ..." form.
  expect(JSON.parse(written.get('output/result_c_scenario_1_1/target-run.json')!).error).toBe(
    "Request was considered invalid due to error:\nImmersion heater references an undeclared energy supply 'ghost supply'.");
});

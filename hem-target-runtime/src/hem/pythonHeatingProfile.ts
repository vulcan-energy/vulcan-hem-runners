// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { pythonFhsArguments } from './pythonFhsArguments';
import { runHeatingProfile, type HeatingProfile } from './heatingProfile';

export interface PythonStageRuntime {
  runPythonAsync(code: string): Promise<unknown>;
  FS: { writeFile(path: string, data: string): void; readdir(path: string): string[]; readFile(path: string, options: { encoding: 'utf8' }): string; };
}
/** Uses the public FHS and core CLIs from the already loaded, version-verified environment. */
export async function executePythonHeatingProfile(py: PythonStageRuntime, folder: string, input: string, profile: HeatingProfile, weatherPath?: string): Promise<Record<string, string>> {
  return runHeatingProfile(input, profile, async (data, stage) => {
    const stem = stage === 'preprocess' ? 'model' : 'heating';
    const path = `${folder}/${stem}.json`;
    py.FS.writeFile(path, data);
    if (stage === 'preprocess') {
      const args = [...pythonFhsArguments(path, ['actual'], weatherPath), '--preprocess-only'];
      await py.runPythonAsync(`main(${JSON.stringify(args)}, standalone_mode=False)`);
    } else {
      const args = [path, '--heat-balance', '--detailed-output-heating-cooling', '--fail-fast'];
      await py.runPythonAsync(`from hem_core.hem import main as run_core\ncore_status = run_core(${JSON.stringify(args)})\nif core_status != 0:\n    raise RuntimeError(f"Core replay failed with status {core_status}")`);
    }
    const output = `${folder}/${stem}__results`;
    return Object.fromEntries(py.FS.readdir(output).filter(name => name !== '.' && name !== '..').map(name => [name, py.FS.readFile(`${output}/${name}`, { encoding: 'utf8' })]));
  });
}

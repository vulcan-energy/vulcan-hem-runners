// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

/** Exact Click spelling in pinned Python FHS a8/a9; persisted modes stay canonical. */
export function pythonFhsArguments(inputPath: string, modes: string[], weatherPath?: string): string[] {
  const cliModes: Record<string, string> = {
    actual: 'actual', 'actual-fee': 'actual-FEE', notional: 'notional', 'notional-fee': 'notional-FEE',
  };
  return [inputPath, ...(weatherPath ? ['--epw-file', weatherPath] : []), ...modes.flatMap(mode => {
    if (!Object.prototype.hasOwnProperty.call(cliModes, mode)) throw new Error(`Unsupported Python FHS mode ${mode}`);
    return ['--mode', cliModes[mode]];
  })];
}

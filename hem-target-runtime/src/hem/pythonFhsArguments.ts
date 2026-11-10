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

const MAX_DETAIL_LINE = 200;

/**
 * The final exception of a Pyodide traceback, for people: its header plus any detail lines (the
 * field and reason of a validation error), on one line. Text with no traceback is already a message.
 */
export function pythonFailureMessage(text: string): string {
  const lastFrame = text.lastIndexOf('\n  File "');
  if (lastFrame < 0 || !text.includes('Traceback (most recent call last)')) return text;
  const lines = text.slice(lastFrame + 1).split('\n').slice(1);
  const header = lines.findIndex(line => line.trim() && !/^\s/.test(line));
  if (header < 0) return text;
  // Keep each field path and reason; drop pydantic's dump of the offending input and cap the length.
  const exception = lines.slice(header).map(line => line.trim().replace(/, input_value=.*$/, ']'))
    .filter(line => line && !line.startsWith('For further information visit'))
    .map(line => (line.length > MAX_DETAIL_LINE ? `${line.slice(0, MAX_DETAIL_LINE - 1)}…` : line));
  return `HEM could not calculate this model. ${exception.join(' ')}`;
}

// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { describe, expect, it } from 'vitest';
import { pythonFhsArguments } from './pythonFhsArguments';

describe('pinned Python FHS CLI modes', () => {
  it('translates canonical FEE mode identities to the exact case-sensitive Click values', () => {
    expect(pythonFhsArguments('/run/model.json', ['actual', 'actual-fee', 'notional', 'notional-fee'])).toEqual([
      '/run/model.json', '--mode', 'actual', '--mode', 'actual-FEE', '--mode', 'notional', '--mode', 'notional-FEE',
    ]);
  });
  it('passes one explicit EPW to the complete mode invocation', () => {
    expect(pythonFhsArguments('/run/model.json', ['actual', 'notional'], '/run/weather.epw')).toEqual(['/run/model.json', '--epw-file', '/run/weather.epw', '--mode', 'actual', '--mode', 'notional']);
  });
  it('rejects unsupported modes instead of passing an accidental default', () => {
    expect(() => pythonFhsArguments('/run/model.json', ['unknown'])).toThrow('Unsupported');
  });
});

// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { describe, expect, it } from 'vitest';
import { pythonFailureMessage, pythonFhsArguments } from './pythonFhsArguments';

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

describe('Python failure message', () => {
  const traceback = (exception: string) => `PythonError: Traceback (most recent call last):\n  File "/lib/python/site-packages/bin/fhs.py", line 40, in main\n    run(args)\n    ^^^^^^^^^\n  File "/lib/python/site-packages/future_homes_standard/x.py", line 9, in run\n    project["other"]\n${exception}\n`;
  it('shows only the final exception line of a KeyError', () => {
    expect(pythonFailureMessage(traceback("KeyError: 'other'"))).toBe("HEM could not calculate this model. KeyError: 'other'");
  });
  it('shows only the final exception line of a ValueError', () => {
    expect(pythonFailureMessage(traceback('ValueError: Invalid number of bedrooms: 0'))).toBe('HEM could not calculate this model. ValueError: Invalid number of bedrooms: 0');
  });
  it('keeps the header and the field detail lines of a multi-line exception', () => {
    expect(pythonFailureMessage(traceback('pydantic_core._pydantic_core.ValidationError: 1 validation error for Input\nSpaceHeatSystem.x.fan_pwr\n  Field required [type=missing]\n    For further information visit https://errors.pydantic.dev')))
      .toBe('HEM could not calculate this model. pydantic_core._pydantic_core.ValidationError: 1 validation error for Input SpaceHeatSystem.x.fan_pwr Field required [type=missing]');
  });
  it('drops the input value dump and caps each detail line', () => {
    const reason = `Value error, ${'x'.repeat(400)}`;
    const message = pythonFailureMessage(traceback(`pydantic_core._pydantic_core.ValidationError: 1 validation error for Input\nZone.Zone 1.Window\n  ${reason} [type=value_error, input_value={'type': 'BuildingElement...]'}, input_type=dict]`));
    expect(message).not.toContain('input_value');
    expect(message).toContain('Zone.Zone 1.Window Value error, xxx');
    expect(message.split(' Zone.Zone 1.Window ')[1].length).toBeLessThanOrEqual(200);
  });
  it('leaves a message with no traceback alone', () => {
    expect(pythonFailureMessage('Error: Python worker is not initialized')).toBe('Error: Python worker is not initialized');
  });
});

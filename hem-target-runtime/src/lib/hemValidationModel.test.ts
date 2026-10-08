// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { describe, expect, it } from 'vitest';

import { cloneForHemValidationJson } from './hemValidationModel';

describe('cloneForHemValidationJson', () => {
  it('removes only the named converter-owned field from the FHS preflight clone', () => {
    const productModelJson = JSON.stringify({
      General: {
        built_form: 2,
        build_type: 'house',
        not_a_converter_extension: true,
      },
      built_form: 6,
      Zone: { Living: { volume: 100 } },
    });

    const validationModel = JSON.parse(cloneForHemValidationJson(productModelJson));

    expect(validationModel).toEqual({
      General: {
        build_type: 'house',
        not_a_converter_extension: true,
      },
      built_form: 6,
      Zone: { Living: { volume: 100 } },
    });
    expect(JSON.parse(productModelJson).General.built_form).toBe(2);
  });

  it('fails explicitly when the converted product model is not an object', () => {
    expect(() => cloneForHemValidationJson('null')).toThrow(
      'Converted product model must be a JSON object',
    );
  });
});

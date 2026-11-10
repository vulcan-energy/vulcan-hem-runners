// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { correctTargetSchema } from './targetSchemaCorrections';

const elementRules = (schema: any) => schema.properties.Zone.additionalProperties.properties.BuildingElement.additionalProperties.allOf;

describe.each(['python-fhs-a8-f2ab6cf7-8ca182b0-v1', 'python-fhs-a9-918addad-b9b90138-v1'])('%s schema correction', id => {
  const original = readFileSync(resolve(__dirname, `../../../fixtures/hem_target_schemas/${id}.schema.json`), 'utf8');

  it('applies only the listed corrections', () => {
    const expected = JSON.parse(original);
    expected.properties.Zone.additionalProperties.properties.BuildingElement.additionalProperties.allOf[0].then.required =
      ['area', 'areal_heat_capacity', 'mass_distribution_class', 'pitch'];
    const [storage, , , warmAir] = expected.properties.SpaceHeatSystem.additionalProperties.allOf;
    warmAir.then.required = ['frac_convective', 'HeatSource'];
    warmAir.then.properties.temp_diff_emit_dsgn = false;
    storage.then.required = [...storage.then.required, 'fan_pwr'];
    storage.then.properties.fan_pwr = { type: 'number', minimum: 0 };
    const corrected = JSON.parse(correctTargetSchema(id, original));
    expect(corrected).toEqual(expected);
    expect(elementRules(corrected)[1].then.required).toEqual(['is_adjacent_space_within_dwelling']);
  });

  it('finds the space heating rules by type, not position', () => {
    const reordered = JSON.parse(original);
    reordered.properties.SpaceHeatSystem.additionalProperties.allOf.reverse();
    const corrected = JSON.parse(correctTargetSchema(id, JSON.stringify(reordered)));
    const rules = corrected.properties.SpaceHeatSystem.additionalProperties.allOf;
    expect(rules[0].then.properties.temp_diff_emit_dsgn).toBe(false);
    expect(rules[3].then.required).toContain('fan_pwr');
  });

  it('fails loudly once the pinned schema no longer has the defect', () => {
    expect(() => correctTargetSchema(id, correctTargetSchema(id, original))).toThrow('no longer applies');
  });
});

describe('rust-fhs-a7-62d3df70-c5ba2673-v1 schema correction', () => {
  const id = 'rust-fhs-a7-62d3df70-c5ba2673-v1';
  const original = readFileSync(resolve(__dirname, '../../../hem_fhs_upstream/schema/input_fhs.schema.json'), 'utf8');

  it('removes reveal from window shading only, not from PV shading', () => {
    const corrected = JSON.parse(correctTargetSchema(id, original));
    const window = elementRules(corrected)[5];
    expect(window.if).toEqual({ properties: { type: { const: 'BuildingElementTransparent' } } });
    expect(window.then.properties.shading.items.properties.type.enum).toEqual(['obstacle', 'overhang', 'sidefinleft', 'sidefinright']);
    expect(corrected.properties.OnSiteGeneration.additionalProperties.properties.shading).toEqual({ $ref: '#/$defs/WindowShading' });
    expect(corrected.$defs.WindowShading).toEqual(JSON.parse(original).$defs.WindowShading);
  });

  it('fails loudly once the pinned schema no longer has the defect', () => {
    expect(() => correctTargetSchema(id, correctTargetSchema(id, original))).toThrow('no longer applies');
  });
});

it('leaves unknown targets byte-identical', () => {
  expect(correctTargetSchema('unknown-target', '{ "unparsed": true }')).toBe('{ "unparsed": true }');
});

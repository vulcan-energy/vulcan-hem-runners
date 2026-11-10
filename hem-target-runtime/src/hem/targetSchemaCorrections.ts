// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

/**
 * Temporary corrections of upstream schema defects. Remove them when a pin ships the upstream fix.
 * See "Schema correction" and "Further upstream mismatches" in
 * docs/development/HEM_Python_A9_Qualification.md.
 *
 * FHS 1.0.0a8 and 1.0.0a9:
 * - require `is_adjacent_space_within_dwelling` for both adjacent element types but declare it only
 *   for conditioned space, so no unconditioned element can validate. Conditioned elements stay
 *   covered by the requirement in the next `allOf` branch.
 * - require `temp_diff_emit_dsgn` on WarmAir, which core's `SpaceHeatSystemWarmAir` forbids.
 * - declare `fan_pwr` on ElecStorageHeater only for fan-assisted units, which core requires for
 *   every unit. A damper-only unit fails with the field and without it.
 *
 * Rust FHS 1.0.0a7:
 * - accepts `reveal` in a window's `shading`, but the wrapper passes the reveal to the engine's
 *   window shading calculation unexpanded, and every run fails with "unexpected window shading
 *   object type encountered". A PV `shading` reveal runs, so only the window loses `reveal`.
 *   Remove this entry when the a7 pin is retired or ships a wrapper that expands a reveal.
 */
const PYTHON_TARGETS = new Set(['python-fhs-a8-f2ab6cf7-8ca182b0-v1', 'python-fhs-a9-918addad-b9b90138-v1']);
const RUST_A7 = 'rust-fhs-a7-62d3df70-c5ba2673-v1';
const FIELD = 'is_adjacent_space_within_dwelling';
const WARM_AIR_FIELD = 'temp_diff_emit_dsgn';
const STORAGE_FAN_FIELD = 'fan_pwr';

/**
 * Whether correctTargetSchema changes this target's schema. The editor and preparation read every
 * target through correctTargetSchema; the Python worker also writes the corrected schema over the
 * wrapper's own copy, which only the Python targets (a8, a9) validate against.
 */
export const hasTargetSchemaCorrection = (targetId: string): boolean => PYTHON_TARGETS.has(targetId) || targetId === RUST_A7;

type Rule = { if?: { properties?: { type?: { const?: string } }; anyOf?: Rule['if'][] }; then: { required: string[]; properties: Record<string, unknown> } };
const ruleTypes = (rule: Rule): string[] =>
  [rule.if?.properties?.type?.const, ...(rule.if?.anyOf ?? []).map(branch => branch?.properties?.type?.const)].filter((type): type is string => !!type);

/** The `then` of the one `allOf` rule whose `if` names exactly these `type` consts, in any order. */
function thenFor(targetId: string, rules: Rule[], ...types: string[]) {
  const matches = rules.filter(rule => { const own = ruleTypes(rule); return own.length === types.length && types.every(type => own.includes(type)); });
  if (matches.length !== 1) throw new Error(`Schema correction for ${targetId} no longer applies: ${matches.length} rules for ${types.join(' or ')}`);
  return matches[0].then;
}

export function correctTargetSchema(targetId: string, schemaText: string): string {
  if (!hasTargetSchemaCorrection(targetId)) return schemaText;
  const schema = JSON.parse(schemaText);
  const elementRules = schema.properties.Zone.additionalProperties.properties.BuildingElement.additionalProperties.allOf;
  if (targetId === RUST_A7) {
    const window = thenFor(targetId, elementRules, 'BuildingElementTransparent');
    const shared = schema.$defs.WindowShading;
    const types: string[] = shared.items.properties.type.enum;
    if ((window.properties.shading as { $ref?: string } | undefined)?.$ref !== '#/$defs/WindowShading' || !types.includes('reveal')) {
      throw new Error(`Schema correction for ${targetId} no longer applies`);
    }
    const shading = structuredClone(shared);
    shading.items.properties.type.enum = types.filter(type => type !== 'reveal');
    window.properties.shading = shading;
    return JSON.stringify(schema);
  }
  const branch = thenFor(targetId, elementRules,
    'BuildingElementAdjacentConditionedSpace', 'BuildingElementAdjacentUnconditionedSpace_Simple');
  const spaceHeatRules = schema.properties.SpaceHeatSystem.additionalProperties.allOf;
  const storage = thenFor(targetId, spaceHeatRules, 'ElecStorageHeater');
  const warmAir = thenFor(targetId, spaceHeatRules, 'WarmAir');
  if (!branch.required.includes(FIELD) || !warmAir.required.includes(WARM_AIR_FIELD) || storage.properties[STORAGE_FAN_FIELD]) {
    throw new Error(`Schema correction for ${targetId} no longer applies`);
  }
  branch.required = branch.required.filter((key: string) => key !== FIELD);
  warmAir.required = warmAir.required.filter((key: string) => key !== WARM_AIR_FIELD);
  warmAir.properties[WARM_AIR_FIELD] = false;
  storage.properties[STORAGE_FAN_FIELD] = { type: 'number', minimum: 0 };
  storage.required = [...storage.required, STORAGE_FAN_FIELD];
  return JSON.stringify(schema);
}

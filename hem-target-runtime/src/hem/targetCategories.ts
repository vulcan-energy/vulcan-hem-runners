// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { formatCategoryName } from '../utils/textFormatting';
import { zoneAllocationError } from '../lib/zoneAllocations';

/**
 * Scenario categories pinned targets cannot prepare: execution flags the a7 runtime fixes
 * (SAP XML, extra outputs), snippets whose shape no FHS target accepts (lighting, tariff,
 * controls), structural replacements (space heat systems), area rescaling (glazing), and
 * internal gains, which FHS derives itself so the snippet would silently change nothing.
 */
export const TARGET_UNSUPPORTED_CATEGORIES = new Set([
  'sap_xml', 'additional_outputs', 'lighting', 'glazing', 'space_heat_systems', 'tariff', 'controls', 'internal_gains',
]);

/** Selections a scenario must change before it can be saved for a pinned target. */
export function scenarioSelectionErrors(scenario: { name: string; parameters: Record<string, string[]>; zone_allocations?: Record<string, Record<string, string[]>> }): string[] {
  const zoneError = zoneAllocationError(scenario);
  const errors = zoneError ? [zoneError] : [];
  for (const [category, choices] of Object.entries(scenario.parameters)) {
    if (choices.length > 0 && TARGET_UNSUPPORTED_CATEGORIES.has(category)) {
      errors.push(`${scenario.name}: ${formatCategoryName(category)} can't run with HEM model versions yet — uncheck it to save.`);
    }
  }
  return errors;
}

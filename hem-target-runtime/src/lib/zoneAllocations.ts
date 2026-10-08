// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

/**
 * Zones that actually allocate something. Zone detection seeded an empty entry per zone
 * ({"Zone 1": {}}) for the editor; those are not allocations and are never saved.
 */
export function allocatedZones<T>(zoneAllocations: Record<string, Record<string, T[] | T>> | null | undefined) {
  const allocated = Object.fromEntries(
    Object.entries(zoneAllocations ?? {})
      .map(([zone, categories]) => [zone, Object.fromEntries(
        Object.entries(categories ?? {}).filter(([, choice]) => (Array.isArray(choice) ? choice.length > 0 : Boolean(choice))),
      )] as const)
      .filter(([, categories]) => Object.keys(categories).length > 0),
  );
  return Object.keys(allocated).length > 0 ? allocated : undefined;
}

type ZoneAllocations = Record<string, Record<string, string[]>>;

/**
 * Reads one saved scenario. Scenarios apply snippets to the whole model, so per-zone
 * selections from older configs fold into parameters when every allocated zone made the
 * same choices. Otherwise they are returned unchanged for `zoneAllocationError`.
 */
export function savedScenarioSelections(value: Record<string, unknown>): { parameters: Record<string, string[]>; zone_allocations?: ZoneAllocations } {
  const parameters: Record<string, string[]> = {};
  for (const [category, choices] of Object.entries(value)) {
    if (Array.isArray(choices)) parameters[category] = choices;
  }
  const allocated = allocatedZones(value.zone_allocations as Record<string, Record<string, string[] | string>> | undefined);
  if (!allocated) return { parameters };
  const zones: ZoneAllocations = Object.fromEntries(Object.entries(allocated).map(([zone, categories]) => [
    zone,
    Object.fromEntries(Object.entries(categories).map(([category, choice]) => [category, [choice].flat().sort()])),
  ]));
  const signatures = new Set(Object.values(zones).map(categories => JSON.stringify(Object.entries(categories).sort())));
  if (signatures.size > 1) return { parameters, zone_allocations: zones };
  for (const [category, choices] of Object.entries(Object.values(zones)[0])) {
    parameters[category] = [...new Set([...(parameters[category] ?? []), ...choices])];
  }
  return { parameters };
}

export function zoneAllocationError(scenario: { name: string; zone_allocations?: ZoneAllocations }): string | undefined {
  if (!scenario.zone_allocations) return undefined;
  const zones = Object.entries(scenario.zone_allocations).map(([zone, categories]) =>
    `${zone}: ${Object.entries(categories).map(([category, choices]) => `${category} ${choices.join(', ')}`).join('; ')}`);
  return `${scenario.name} was saved with different per-zone selections (${zones.join(' | ')}). Scenarios now apply to the whole model: select those parameters again, then save.`;
}

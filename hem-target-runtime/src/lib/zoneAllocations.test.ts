// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { expect, it } from 'vitest';
import { allocatedZones } from './zoneAllocations';

it('drops empty zone entries and keeps real allocations', () => {
  expect(allocatedZones({ 'Zone 1': {} })).toBeUndefined();
  expect(allocatedZones({ 'Zone 1': { control: [] } })).toBeUndefined();
  expect(allocatedZones(undefined)).toBeUndefined();
  expect(allocatedZones({ 'Zone 1': {}, 'Zone 2': { control: ['eco'], space_heat_emitters: [] } }))
    .toEqual({ 'Zone 2': { control: ['eco'] } });
});

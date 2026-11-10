// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { webcrypto } from 'node:crypto';
import { csvTarget, ecaasOnlyBatchMessage, treatedFloorAreaBatchMessage, snapshotBatchTarget, prepareTargetBatch, prepareTargetCsvWithManifest, withPythonFanPositionErrors, authoredOmissionWarning, withFhsInputErrors } from './targetPreparation';
import ecaasSchema from '../../../schemas/ecaas_input_fhs.schema.json?raw';
import { HEM_TARGETS, type HemTargetManifest } from './targets';
import { TargetAssets } from './targetAssets';
import { correctTargetSchema } from './targetSchemaCorrections';
import { TARGET_UNSUPPORTED_CATEGORIES } from './targetCategories';

// These tests stub the schema with placeholder text; the correction has its own test on the real schemas.
vi.mock('./targetSchemaCorrections', () => ({ correctTargetSchema: vi.fn((_id: string, schema: string) => schema) }));
beforeEach(() => vi.stubGlobal('crypto', webcrypto));
describe('treated floor area guard', () => {
  const csv = (labelDepth: number) => [
    'Zone,,,,,,,,,,,,,',
    'Name,Type,volume,floor_area,height,simplified thermal bridging',
    'Living,Zone,21.6,9,2.4,FALSE',
    '',
    'Ground Elements,,,,,,,,,,,,,',
    'Name,Zone,Type,area,width,height,perimeter,floor_type,depth_basement_floor,thickness_walls,base_height,parent_element,coords,extra_json',
    'Ground Floor,Living,BuildingElementGround,9,3,0.2,12,Slab_no_edge_insulation,,,0,,"0,0,0|3,0,0|3,3,0|0,3,0",',
    '',
    'Space Labels,,,,,,',
    'Name,Zone,storey,room_type,coords,height_override,extra_json',
    `Bedroom 1,Living,0,bedroom,"0,0,0|3,0,0|3,${labelDepth},0|0,${labelDepth},0",,`,
  ].join('\n');
  const snapshot = { targetBundleId: 't', sources: { bad: { csv: csv(20), sha256: '' }, good: { csv: csv(3), sha256: '' } }, snippets: {} };
  it('stops a saved batch whose model loses material treated floor area', () => {
    expect(treatedFloorAreaBatchMessage({ one: { base_json: ['bad'] } }, snapshot)).toContain('bad: Z=0: 51.0 m² of space labels are not over a treated floor');
    expect(treatedFloorAreaBatchMessage({ one: { base_json: ['good'] } }, snapshot)).toBeUndefined();
  });
  it('checks the zone floor area against the labels whatever ComplianceValidationEnabled says', () => {
    const source = `Metadata,\nComplianceValidationEnabled,FALSE\n\n${csv(3).replace('Living,Zone,21.6,9,', 'Living,Zone,48,20,')}`;
    const off = { ...snapshot, sources: { off: { csv: source, sha256: '' } } };
    expect(treatedFloorAreaBatchMessage({ one: { base_json: ['off'] } }, off)).toMatch(/^off: /);
  });
  it('does not parse a source that has no space labels', () => {
    const malformed = { targetBundleId: 't', sources: { old: { csv: 'Exposed Elements,,\nName,Zone\nWall,Z\n', sha256: '' } }, snippets: {} };
    expect(treatedFloorAreaBatchMessage({ one: { base_json: ['old'] } }, malformed)).toBeUndefined();
  });
});
describe('saved batch target snapshots', () => {
  it('freezes source and snippet bytes with exact target identity', async () => {
    const csv = `#Metadata\nVulcanCsvVersion,3\nTargetBundleId,${HEM_TARGETS[1].id}\n`;
    const snippet = '{"value":2}';
    const read = vi.fn(async (path: string) => path.endsWith('.csv') ? csv : snippet);
    const snapshot = await snapshotBatchTarget({ one: { base_json: ['model'], airtightness: ['small'], model_wrappers: ['fhs_assumptions'] } }, read);
    expect(snapshot.targetBundleId).toBe(HEM_TARGETS[1].id);
    expect(snapshot.sources.model.csv).toBe(csv);
    expect(snapshot.snippets['input/batch_parameters/airtightness/small.json'].json).toBe(snippet);
    expect(read).toHaveBeenCalledTimes(2);
  });
  it('requires an explicit pin for legacy files and rejects mixed models', async () => {
    await expect(snapshotBatchTarget({ one: { base_json: ['legacy'] } }, async () => '#Metadata\nVulcanCsvVersion,2')).rejects.toThrow('Open and save each base model');
    await expect(snapshotBatchTarget({ one: { base_json: ['a', 'b'] } }, async path => `TargetBundleId,${path.includes('/a.csv') ? HEM_TARGETS[0].id : HEM_TARGETS[1].id}`)).rejects.toThrow('cannot mix');
  });
  it('refuses categories a pinned target cannot run, as the app does at save', async () => {
    const read = async (path: string) => path.endsWith('.csv') ? `TargetBundleId,${HEM_TARGETS[0].id}\n` : '{}';
    await expect(snapshotBatchTarget({ scenario_1: { base_json: ['model'], space_heat_systems: ['heater'] } }, read))
      .rejects.toThrow("scenario_1: Space Heat Systems can't run with HEM model versions yet — remove it from the scenario, then save the batch.");
  });
  it('handles quoted metadata and rejects duplicate pins', () => {
    expect(csvTarget('"TargetBundleId","pin"')).toBe('pin');
    expect(() => csvTarget('TargetBundleId,a\nTargetBundleId,b')).toThrow('conflicting');
  });
});

it('keeps configuration category order for scenario indices used by Results', async () => {
  const target = HEM_TARGETS[1];
  const csv = `TargetBundleId,${target.id}\n`;
  const config = { __hem: { targetBundleId: target.id }, one: { base_json: ['model'], windows: ['w1', 'w2'], airtightness: ['a1', 'a2'] } };
  const snapshot = await snapshotBatchTarget(config, async path => path.endsWith('.csv') ? csv : JSON.stringify({ choice: path.split('/').pop() }));
  const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async () => new TextEncoder().encode('{}') } as unknown as TargetAssets);
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const prepare = vi.fn(() => JSON.stringify({ ok: true, output: { model: {}, validation: { is_valid: true, errors: [] } } }));
  try {
    const jobs = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare);
    expect(jobs.map(job => job.name)).toEqual(['one_0', 'one_1', 'one_2', 'one_3']);
    expect(new Set(jobs.map(job => job.startedAt))).toEqual(new Set(['2026-01-01T00:00:00.000Z']));
    expect(prepare.mock.calls.map(call => JSON.parse(call[1]).map(([category, value]: [string, { choice: string }]) => `${category}:${value.choice}`))).toEqual([
      ['windows:w1.json', 'airtightness:a1.json'], ['windows:w1.json', 'airtightness:a2.json'],
      ['windows:w2.json', 'airtightness:a1.json'], ['windows:w2.json', 'airtightness:a2.json'],
    ]);
  } finally { assets.mockRestore(); }
});

it('omits a cylinder init_temp the target Tank does not declare, with a run warning', async () => {
  const target = HEM_TARGETS[1];
  const config = { __hem: { targetBundleId: target.id }, one: { base_json: ['model'], hot_water_source: ['cylinder'] } };
  const csv = `TargetBundleId,${target.id}\n`;
  const snippet = JSON.stringify({ HotWaterSource: { 'hw cylinder': { type: 'StorageTank', init_temp: 55 } }, PreHeatedWaterSource: { 'preheated tank': { init_temp: 55 } } });
  const snapshot = await snapshotBatchTarget(config, async path => path.endsWith('.csv') ? csv : snippet);
  const schemaText = (tank: Record<string, unknown>) => JSON.stringify({ $defs: { Tank: { properties: tank } } });
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const run = async (tank: Record<string, unknown>) => {
    const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async (artifact: { path: string }) => new TextEncoder().encode(artifact.path === 'schema.json' ? schemaText(tank) : '{}') } as unknown as TargetAssets);
    const prepare = vi.fn<(request: string, snippets: string) => string>(() => JSON.stringify({ ok: true, output: { model: {}, validation: { is_valid: true, errors: [] } } }));
    try {
      const [job] = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare);
      return { snippets: JSON.parse(prepare.mock.calls[0][1])[0][1], warnings: job.warnings };
    } finally { assets.mockRestore(); }
  };
  const absent = await run({});
  expect(absent.snippets.HotWaterSource['hw cylinder']).toEqual({ type: 'StorageTank' });
  expect(absent.snippets.PreHeatedWaterSource['preheated tank'].init_temp).toBe(55);
  expect(absent.warnings).toEqual([expect.stringContaining('init_temp')]);
  const declared = await run({ init_temp: {} });
  expect(declared.snippets.HotWaterSource['hw cylinder'].init_temp).toBe(55);
  expect(declared.warnings).toBeUndefined();
});

it('omits a compliance count the target schema lacks and keeps it where the schema has it', async () => {
  const target = HEM_TARGETS[1];
  const config = { one: { base_json: ['model'], compliance_settings: ['two_bed'] } };
  const csv = `TargetBundleId,${target.id}\n`;
  const snapshot = await snapshotBatchTarget(config, async path => path.endsWith('.csv') ? csv : JSON.stringify({ NumberOfBedrooms: 2, NumberOfSanitaryAccommodations: 0 }));
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const run = async (properties: Record<string, unknown>) => {
    const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async (artifact: { path: string }) => new TextEncoder().encode(artifact.path === 'schema.json' ? JSON.stringify({ properties: { General: {}, ...properties } }) : '{}') } as unknown as TargetAssets);
    const prepare = vi.fn<(request: string, snippets: string) => string>(() => JSON.stringify({ ok: true, output: { model: {}, validation: { is_valid: true, errors: [] } } }));
    try {
      const [job] = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare);
      return { snippet: JSON.parse(prepare.mock.calls[0][1])[0][1], warnings: job.warnings };
    } finally { assets.mockRestore(); }
  };
  const absent = await run({ NumberOfBedrooms: {} });
  expect(absent.snippet).toEqual({ NumberOfBedrooms: 2 });
  expect(absent.warnings).toEqual([expect.stringContaining('NumberOfSanitaryAccommodations')]);
  const declared = await run({ NumberOfBedrooms: {}, NumberOfSanitaryAccommodations: {} });
  expect(declared.snippet).toEqual({ NumberOfBedrooms: 2, NumberOfSanitaryAccommodations: 0 });
  expect(declared.warnings).toBeUndefined();
});

it('names only authored fields among schema omissions', () => {
  const omissions = ['/HotWaterSource/hw cylinder/init_temp', '/InfiltrationVentilation/Vents/V1/pressure_difference_ref', '/Zone/Z/BuildingElement/W/geometry_face', '/Zone/Z/BuildingElement/W/_treatment_ui'].map(path => ({ code: 'TARGET_FIELD_OMITTED', path }));
  const csv = 'x,"{""init_temp"":55,""geometry_face"":1,""_treatment_ui"":{}}"\n';
  expect(authoredOmissionWarning(omissions, csv, '1.0.0a9')).toBe('HEM 1.0.0a9 does not use init_temp from this model. Omitted from the run; the source keeps it.');
  expect(authoredOmissionWarning([{ code: 'W_TARGET_INPUT', path: '/a/init_temp' }], csv, '1.0.0a9')).toBeUndefined();
  expect(authoredOmissionWarning(undefined, csv, '1.0.0a9')).toBeUndefined();
});

describe('inputs FHS cannot run', () => {
  const valid = (model: unknown) => ({ model, validation: { is_valid: true, errors: [] as unknown[] } });
  const shower = { HotWaterDemand: { Shower: { s: {} }, Bath: {} } };
  const counted = 'NumberOfBedrooms,2\nNumberOfWetRooms,1\n';
  it('stops a model with no shower and no bath', () => {
    expect(withFhsInputErrors(valid({ HotWaterDemand: { Shower: {}, Bath: {} } }), counted).validation.errors).toEqual([expect.objectContaining({ path: '/HotWaterDemand/Shower', code: 'E_FHS_NO_SHOWER_OR_BATH' })]);
    expect(withFhsInputErrors(valid(shower), counted).validation.is_valid).toBe(true);
    expect(withFhsInputErrors(valid({ HotWaterDemand: { Bath: { b: {} } } }), counted).validation.is_valid).toBe(true);
  });
  it('stops a room count the template would silently supply', () => {
    const result = withFhsInputErrors(valid({ ...shower, NumberOfBedrooms: 1, NumberOfWetRooms: 1 }), 'NumberOfBedrooms,\n');
    expect(result.validation.is_valid).toBe(false);
    expect(result.validation.errors.map(error => (error as { path: string }).path)).toEqual(['/NumberOfBedrooms', '/NumberOfWetRooms']);
  });
  it('accepts a count a scenario snippet authored', () => {
    expect(withFhsInputErrors(valid({ ...shower, NumberOfBedrooms: 3 }), 'NumberOfWetRooms,1\n', new Set(['NumberOfBedrooms'])).validation.is_valid).toBe(true);
  });
});

it('treats events like internal gains: FHS derives its own', () => {
  expect(TARGET_UNSUPPORTED_CATEGORIES.has('events')).toBe(true);
});

it('captures declared defaults and refuses a customized canonical template instead of discarding it', async () => {
  const target = HEM_TARGETS[1];
  const csv = `Metadata\nTargetBundleId,${target.id}\n"DefaultsPath","input/defaults/defaults_template.json"\n`;
  const config = { one: { base_json: ['model'] } };
  const snapshot = await snapshotBatchTarget(config, async path => path.endsWith('.csv') ? csv : '{"authored":2}');
  expect(snapshot.sources.model.defaults?.json).toBe('{"authored":2}');
  const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async () => new TextEncoder().encode('{}') } as unknown as TargetAssets);
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const prepare = vi.fn();
  try {
    await expect(prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare)).rejects.toThrow('differ from this target');
    expect(prepare).not.toHaveBeenCalled();
    snapshot.sources.model.defaults!.json = '{}';
    await expect(prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare)).rejects.toThrow('changed saved defaults');
  } finally { assets.mockRestore(); }
});

it('uses a saved matching defaults snapshot without reading mutable workspace files during preparation', async () => {
  const target = HEM_TARGETS[1];
  const config = { one: { base_json: ['model'] } };
  const csv = `Metadata\nTargetBundleId,${target.id}\nDefaultsPath,input/defaults/defaults_template.json\n`;
  const read = vi.fn(async (path: string) => path.endsWith('.csv') ? csv : '{}');
  const snapshot = await snapshotBatchTarget(config, read);
  read.mockRejectedValue(new Error('workspace changed'));
  const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async () => new TextEncoder().encode('{}') } as unknown as TargetAssets);
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  try {
    const warning = { code: 'W_TARGET_INPUT', path: '/Zone/Room/BuildingElement/Party/u_value_whole_wall', message: 'HEM 1.0.0a9: stale whole-wall U' };
    vi.mocked(correctTargetSchema).mockClear();
    const jobs = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', () => JSON.stringify({ ok: true, output: { model: {}, validation: { is_valid: true, errors: [] }, schema_omissions: [warning, { ...warning, code: 'omitted', message: 'not a warning' }] } }));
    expect(correctTargetSchema).toHaveBeenCalledWith(target.id, expect.any(String));
    expect(jobs).toHaveLength(1);
    // Scenario warnings are non-blocking and travel with the job's run record.
    expect(jobs[0].warnings).toEqual(['HEM 1.0.0a9: stale whole-wall U']);
    expect(read).toHaveBeenCalledTimes(2);
  } finally { assets.mockRestore(); }
});

it('snapshots selected EPW and descriptor, reuses them after workspace changes, and gives weather precedence', async () => {
  const target = HEM_TARGETS[1];
  const config = { one: { base_json: ['model'], weather_files: ['custom'], external_conditions: ['other'], model_wrappers: ['fhs_compliance'] } };
  const files: Record<string, string> = {
    'input/base_models/model.csv': `TargetBundleId,${target.id}\n`,
    'input/batch_parameters/weather_files/custom.json': '{"weather_file":"local.epw"}',
    'input/batch_parameters/weather_files/local.epw': 'LOCATION,synthetic\r\nsmall weather input\r\n',
    // Overridden external_conditions file is deliberately absent.
  };
  const read = vi.fn(async (path: string) => { if (!(path in files)) throw new Error(path); return files[path]; });
  const snapshot = await snapshotBatchTarget(config, read);
  read.mockRejectedValue(new Error('workspace changed'));
  const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async () => new TextEncoder().encode('{}') } as unknown as TargetAssets);
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const prepare = vi.fn(() => JSON.stringify({ ok: true, output: { model: {}, validation: { is_valid: true, errors: [] } } }));
  try {
    const jobs = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare);
    expect(jobs[0].weather?.epw).toBe(files['input/batch_parameters/weather_files/local.epw']);
    expect(jobs[0].modes).toEqual(['actual', 'actual-fee', 'notional', 'notional-fee']);
    expect(JSON.parse(prepare.mock.calls[0][1])).toEqual([]);
    expect(read).toHaveBeenCalledTimes(3);
    snapshot.weather!.custom.epw = 'changed';
    await expect(prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare)).rejects.toThrow('saved weather');
  } finally { assets.mockRestore(); }
});

it('accepts direct EPW selections and rejects path traversal or conflicting aliases', async () => {
  const csv = `TargetBundleId,${HEM_TARGETS[2].id}\n`;
  const snapshot = await snapshotBatchTarget({ one: { base_json: ['model'], weather_file: ['custom.epw'] } }, async path => path.endsWith('.csv') ? csv : 'synthetic EPW');
  expect(snapshot.weather!['custom.epw'].epw).toBe('synthetic EPW');
  await expect(snapshotBatchTarget({ one: { base_json: ['model'], weather_files: ['custom'] } }, async () => '{"weather_file":"../outside.epw"}')).rejects.toThrow('Invalid weather');
  await expect(snapshotBatchTarget({ one: { base_json: ['model'], weather_file: ['a'], weather_files: ['b'] } }, vi.fn())).rejects.toThrow('both');
});

it('runs scenarios whose zone allocations are empty and refuses real ones', async () => {
  const target = HEM_TARGETS[1];
  const csv = `TargetBundleId,${target.id}\n`;
  const scenario = { base_json: ['model'], airtightness: ['a1'] };
  const empty = { __hem: { targetBundleId: target.id }, one: { ...scenario, zone_allocations: { 'Zone 1': {} } } };
  const allocated = { __hem: { targetBundleId: target.id }, one: { ...scenario, zone_allocations: { 'Zone 1': { airtightness: ['a1'] } } } };
  const read = async (path: string) => path.endsWith('.csv') ? csv : JSON.stringify({ choice: path.split('/').pop() });
  const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async () => new TextEncoder().encode('{}') } as unknown as TargetAssets);
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const prepare = vi.fn(() => JSON.stringify({ ok: true, output: { model: {}, validation: { is_valid: true, errors: [] } } }));
  try {
    const jobs = await prepareTargetBatch(empty, await snapshotBatchTarget(empty, read), manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare);
    expect(jobs.map(job => job.name)).toEqual(['one_0']);
    // Saving now refuses per-zone selections, so this is a config saved before that check.
    await expect(prepareTargetBatch(allocated, await snapshotBatchTarget(empty, read), manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare))
      .rejects.toThrow('This Batch Config was saved with per-zone selections. Open it in Scenarios and save it again.');
  } finally { assets.mockRestore(); }
});

describe('ECaaS-only product stop', () => {
  const csv = 'Systems,,\nName,Type,extra_json\nHP,System,"{""HeatSourceWet"":{""hp"":{""type"":""HeatPump"",""product_reference"":""123""}}}"\n';
  const snapshot = { targetBundleId: 't', sources: { ecaas: { csv, sha256: '' } }, snippets: {} };
  it('stops a scenario that runs the product, but not one that replaces HeatSourceWet', () => {
    expect(ecaasOnlyBatchMessage({ one: { base_json: ['ecaas'] } }, snapshot)).toContain('ECaaS-only product (123)');
    expect(ecaasOnlyBatchMessage({ one: { base_json: ['ecaas'], heat_source_wet: ['air_source'] } }, snapshot)).toBeUndefined();
  });
  it('drops only the section a snippet replaces', () => {
    const cylinder = 'Systems,,\nName,Type,extra_json\nHW,System,"{""HotWaterSource"":{""hw cylinder"":{""type"":""StorageTank"",""HeatSource"":{""hwo_hp"":{""type"":""HeatPump_HWOnly"",""product_reference"":""3042""}}}}}"\n';
    const hwo = { targetBundleId: 't', sources: { hwo: { csv: cylinder, sha256: '' } }, snippets: {} };
    expect(ecaasOnlyBatchMessage({ one: { base_json: ['hwo'], heat_source_wet: ['air_source'] } }, hwo)).toContain('ECaaS-only product (3042)');
    expect(ecaasOnlyBatchMessage({ one: { base_json: ['hwo'], hot_water_source: ['cylinder'] } }, hwo)).toBeUndefined();
    expect(ecaasOnlyBatchMessage({ one: { base_json: ['ecaas'], hot_water_source: ['cylinder'] } }, snapshot)).toContain('ECaaS-only product (123)');
  });
});

describe('ECaaS schema selection', () => {
  const hp = 'Systems,,\nName,Type,extra_json\nHP,System,"{""HeatSourceWet"":{""hp"":{""type"":""HeatPump"",""product_reference"":""123""}}}"\n';
  // The preparation module is loaded from a blob URL; a data: module records the request it receives.
  const module = `export default async () => {}; export function prepare_target_scenario(request) {
    globalThis.preparedRequest = JSON.parse(request);
    return JSON.stringify({ ok: true, output: { model: {}, schema_omissions: [], validation: { is_valid: true, errors: [] } } }); }`;
  const prepareCsv = async (target: (typeof HEM_TARGETS)[number], csv: string) => {
    const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json', javascript: 'prep.js', wasm: 'prep.wasm' },
      artifacts: ['schema.json', 'defaults.json', 'prep.js', 'prep.wasm'].map(path => ({ path })) } as unknown as HemTargetManifest;
    const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ download: async () => {},
      bytes: async ({ path }: { path: string }) => new TextEncoder().encode(path === 'schema.json' ? '"target schema"' : '{}') } as unknown as TargetAssets);
    // jsdom has no blob URLs.
    Object.assign(URL, { createObjectURL: () => `data:text/javascript,${encodeURIComponent(module)}`, revokeObjectURL: () => {} });
    try {
      const output = await prepareTargetCsvWithManifest(`TargetBundleId,${target.id}\n${csv}`, manifest);
      return { output, schema: (globalThis as unknown as { preparedRequest: { schema_json: string } }).preparedRequest.schema_json };
    } finally { assets.mockRestore(); }
  };
  const a7 = HEM_TARGETS.find(target => target.version === '1.0.0a7')!;
  const a9 = HEM_TARGETS.find(target => target.version === '1.0.0a9')!;
  it('validates an ECaaS model against the ECaaS schema for its HEM version', async () => {
    const { output, schema } = await prepareCsv(a7, hp);
    expect(schema).toBe(ecaasSchema);
    expect(output.validation.is_valid).toBe(true);
    // A hot-water-only heat pump's reference sits in the cylinder's HeatSource.
    const hwoHp = 'Systems,,\nName,Type,extra_json\nHWO,System,"{""HotWaterSource"":{""hw cylinder"":{""type"":""StorageTank"",""HeatSource"":{""hwo_hp"":{""type"":""HeatPump_HWOnly"",""product_reference"":""3042""}}}}}"\n';
    expect((await prepareCsv(a7, hwoHp)).schema).toBe(ecaasSchema);
  });
  it('reports a missing ECaaS schema instead of validating against the target schema', async () => {
    const { output } = await prepareCsv(a9, hp);
    expect(output.validation).toEqual({ is_valid: false, errors: [expect.objectContaining({ code: 'ecaas_schema_unavailable' })] });
  });
  it('follows the product, not a stale SchemaProfile marker', async () => {
    vi.mocked(correctTargetSchema).mockClear();
    expect((await prepareCsv(a7, 'SchemaProfile,ecaas_input_fhs\n')).schema).toBe('"target schema"');
    expect(correctTargetSchema).toHaveBeenCalledWith(HEM_TARGETS[0].id, '"target schema"');
    // Test data alongside the reference makes it an FHS model.
    const withTestData = hp.replace('""product_reference""', '""test_data_EN14825"":[],""product_reference""');
    const { output, schema } = await prepareCsv(a9, withTestData);
    expect(schema).toBe('"target schema"');
    expect(output.validation.is_valid).toBe(true);
  });
});

// A preparation stand-in: the validator's errors, in the shape finalization.rs emits them, for the model
// prepared alone and for the model with this scenario's snippets.
async function blame(choices: string[], modelErrors: Array<Record<string, string>>, scenarioErrors: Array<Record<string, string>>, target = HEM_TARGETS[0], model = {}) {
  const csv = `TargetBundleId,${target.id}\n`;
  const config = { __hem: { targetBundleId: target.id }, one: { base_json: ['house'], ...Object.fromEntries(choices.map(path => [path.split('/')[0], [path.split('/')[1]]])) } };
  const snapshot = await snapshotBatchTarget(config, async path => path.endsWith('.csv') ? csv : '{}');
  const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async () => new TextEncoder().encode('{}') } as unknown as TargetAssets);
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const prepare = vi.fn((_request: string, snippets: string) => {
    const errors = JSON.parse(snippets).length ? scenarioErrors : modelErrors;
    return JSON.stringify({ ok: true, output: { model, validation: { is_valid: !errors.length, errors } } });
  });
  try {
    return await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare).then(() => 'valid', (error: Error) => error.message);
  } finally { assets.mockRestore(); }
}

it('says which errors the model has without the scenario\'s files, and which occur only with them', async () => {
  const required = { code: 'E026', path: '/InfiltrationVentilation/MechanicalVentilation/Main', message: "'design_outdoor_air_flow_rate' is a required property", keyword: 'required' };
  const maximum = { code: 'E026', path: '/InfiltrationVentilation/MechanicalVentilation/Main/mvhr_eff', message: '2 is greater than the maximum of 1', keyword: 'maximum' };
  // The model lacks a required property of an object the scenario also sets.
  expect(await blame(['mvhr/unit'], [required], [required, maximum])).toBe('Scenario one_0. The model "house" has this error without the scenario\'s parameter files as well: '
    + "'design_outdoor_air_flow_rate' is a required property at /InfiltrationVentilation/MechanicalVentilation/Main. "
    + "This error occurs only with the scenario's parameter files applied (mvhr/unit.json): 2 is greater than the maximum of 1 at /InfiltrationVentilation/MechanicalVentilation/Main/mvhr_eff.");
  // At the root, with two files: neither is named as the cause.
  const root = { code: 'E026', path: '', message: "'SimulationTime' is a required property", keyword: 'required' };
  const extra = { code: 'E026', path: '/InfiltrationVentilation', message: "Additional properties are not allowed ('cross_vent' was unexpected)", keyword: 'additionalProperties' };
  expect(await blame(['airtightness/ap4', 'ventilation/extra'], [root], [root, extra])).toBe('Scenario one_0. The model "house" has this error without the scenario\'s parameter files as well: '
    + "'SimulationTime' is a required property. "
    + "This error occurs only with the scenario's parameter files applied (airtightness/ap4.json, ventilation/extra.json): Additional properties are not allowed ('cross_vent' was unexpected) at /InfiltrationVentilation.");
  // A message quotes the failing object, so a scenario that changes any field of an object the model already fails
  // changes the message; the rule that failed, and where, are the same.
  const oneOf = (object: string) => ({ code: 'E026', path: '/Zone/Room/BuildingElement/North', message: `${object} is not valid under any of the schemas listed in the 'oneOf' keyword`, keyword: 'oneOf' });
  expect(await blame(['airtightness/ap4'], [oneOf('{"pitch":90}')], [oneOf('{"pitch":45}')]))
    .toBe('Scenario one_0. The model "house" has this error without the scenario\'s parameter files as well: {"pitch":45} is not valid under any of the schemas listed in the \'oneOf\' keyword at /Zone/Room/BuildingElement/North.');
  // The same rule failing at another place occurs only with the scenario's files.
  expect(await blame(['airtightness/ap4'], [root], [{ ...root, path: '/InfiltrationVentilation' }]))
    .toBe("Scenario one_0. This error occurs only with the scenario's parameter files applied (airtightness/ap4.json): 'SimulationTime' is a required property at /InfiltrationVentilation.");
});

it('counts the Python fan-position check among the errors the model has without the scenario\'s files', async () => {
  const model = { InfiltrationVentilation: { MechanicalVentilation: { Main: { vent_type: 'Intermittent MEV' } } } };
  expect(await blame(['airtightness/ap4'], [], [], HEM_TARGETS[1], model)).toMatch(/^Scenario one_0\. The model "house" has this error without the scenario's parameter files as well: Fan "Main"/);
});

it('refuses saved selections in categories targets cannot run before preparing', async () => {
  const target = HEM_TARGETS[1];
  const csv = `TargetBundleId,${target.id}\n`;
  const config = { __hem: { targetBundleId: target.id }, one: { base_json: ['model'], internal_gains: ['standard_family'] } };
  const read = async (path: string) => path.endsWith('.csv') ? csv : '{}';
  const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async () => new TextEncoder().encode('{}') } as unknown as TargetAssets);
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const prepare = vi.fn();
  try {
    // Saving now refuses the category, so this is a config saved before that check.
    await expect(prepareTargetBatch(config, await snapshotBatchTarget({ one: { base_json: ['model'] } }, read), manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare))
      .rejects.toThrow("one: Internal Gains can't run with HEM model versions yet — remove it from the scenario, then save the batch.");
    expect(prepare).not.toHaveBeenCalled();
  } finally { assets.mockRestore(); }
});


it('expands saved heating profiles after preparation and rejects compliance combinations', async () => {
  const target = HEM_TARGETS[0];
  const csv = `TargetBundleId,${target.id}\n`;
  const config = { study: { base_json: ['model'], model_wrappers: ['fhs_assumptions'], heating_profiles: ['reference', 'continuous'] } };
  const files: Record<string, string> = {
    'input/base_models/model.csv': csv,
    'input/batch_parameters/heating_profiles/reference.json': '{"type":"reference"}',
    'input/batch_parameters/heating_profiles/continuous.json': '{"type":"weekly","weekday":[[0,24]],"weekend":[[0,24]],"setback":18}',
  };
  const read = async (path: string) => files[path];
  const snapshot = await snapshotBatchTarget(config, read);
  const assets = vi.spyOn(TargetAssets, 'open').mockResolvedValue({ bytes: async () => new TextEncoder().encode('{}') } as unknown as TargetAssets);
  const manifest = { bundleId: target.id, preparation: { schema: 'schema.json', defaults: 'defaults.json' }, artifacts: [{ path: 'schema.json' }, { path: 'defaults.json' }] } as HemTargetManifest;
  const prepare = vi.fn(() => JSON.stringify({ ok: true, output: { model: {}, validation: { is_valid: true, errors: [] } } }));
  try {
    const jobs = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare);
    expect(jobs.map(job => job.heatingProfile?.type)).toEqual(['reference', 'weekly']);
    expect(prepare.mock.calls.map(call => JSON.parse(call[1]))).toEqual([[], []]);
    const invalid = { study: { ...config.study, model_wrappers: ['fhs_compliance'] } };
    await expect(prepareTargetBatch(invalid, snapshot, manifest, 'config', 'run', '2026-01-01T00:00:00.000Z', prepare)).rejects.toThrow('not FHS compliance');
  } finally { assets.mockRestore(); }
});

describe('withPythonFanPositionErrors', () => {
  const output = (fan: Record<string, unknown>) => ({ model: { InfiltrationVentilation: { MechanicalVentilation: { Fan: fan } } }, validation: { is_valid: true, errors: [] as unknown[] } });
  const python = HEM_TARGETS.find((target) => target.runtime === 'python')!;
  const rust = HEM_TARGETS.find((target) => target.runtime === 'rust')!;
  const objectFan = { vent_type: 'Centralised continuous MEV', position_exhaust: { orientation360: 0, pitch: 90, mid_height_air_flow_path: 1.5 } };

  it('reports a Python fan with no flat position as a required field', () => {
    expect(withPythonFanPositionErrors(output(objectFan), python).validation).toMatchObject({
      is_valid: false,
      errors: [expect.objectContaining({ path: '/InfiltrationVentilation/MechanicalVentilation/Fan/orientation360' })],
    });
  });

  it('leaves flat fans, MVHR and the Rust target alone', () => {
    expect(withPythonFanPositionErrors(output({ vent_type: 'Intermittent MEV', orientation360: 0 }), python).validation.is_valid).toBe(true);
    expect(withPythonFanPositionErrors(output({ vent_type: 'MVHR' }), python).validation.is_valid).toBe(true);
    expect(withPythonFanPositionErrors(output(objectFan), rust).validation.is_valid).toBe(true);
  });
});

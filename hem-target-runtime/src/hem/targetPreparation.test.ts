// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { webcrypto } from 'node:crypto';
import { csvTarget, ecaasOnlyBatchMessage, snapshotBatchTarget, prepareTargetBatch, prepareTargetCsvWithManifest } from './targetPreparation';
import ecaasSchema from '../../../schemas/ecaas_input_fhs.schema.json?raw';
import { HEM_TARGETS, type HemTargetManifest } from './targets';
import { TargetAssets } from './targetAssets';
beforeEach(() => vi.stubGlobal('crypto', webcrypto));
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
    const jobs = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', prepare);
    expect(jobs.map(job => job.name)).toEqual(['one_0', 'one_1', 'one_2', 'one_3']);
    expect(prepare.mock.calls.map(call => JSON.parse(call[1]).map(([category, value]: [string, { choice: string }]) => `${category}:${value.choice}`))).toEqual([
      ['windows:w1.json', 'airtightness:a1.json'], ['windows:w1.json', 'airtightness:a2.json'],
      ['windows:w2.json', 'airtightness:a1.json'], ['windows:w2.json', 'airtightness:a2.json'],
    ]);
  } finally { assets.mockRestore(); }
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
    await expect(prepareTargetBatch(config, snapshot, manifest, 'config', 'run', prepare)).rejects.toThrow('differ from this target');
    expect(prepare).not.toHaveBeenCalled();
    snapshot.sources.model.defaults!.json = '{}';
    await expect(prepareTargetBatch(config, snapshot, manifest, 'config', 'run', prepare)).rejects.toThrow('changed saved defaults');
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
    const jobs = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', () => JSON.stringify({ ok: true, output: { model: {}, validation: { is_valid: true, errors: [] }, schema_omissions: [warning, { ...warning, code: 'omitted', message: 'not a warning' }] } }));
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
    const jobs = await prepareTargetBatch(config, snapshot, manifest, 'config', 'run', prepare);
    expect(jobs[0].weather?.epw).toBe(files['input/batch_parameters/weather_files/local.epw']);
    expect(jobs[0].modes).toEqual(['actual', 'actual-fee', 'notional', 'notional-fee']);
    expect(JSON.parse(prepare.mock.calls[0][1])).toEqual([]);
    expect(read).toHaveBeenCalledTimes(3);
    snapshot.weather!.custom.epw = 'changed';
    await expect(prepareTargetBatch(config, snapshot, manifest, 'config', 'run', prepare)).rejects.toThrow('saved weather');
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
    const jobs = await prepareTargetBatch(empty, await snapshotBatchTarget(empty, read), manifest, 'config', 'run', prepare);
    expect(jobs.map(job => job.name)).toEqual(['one_0']);
    await expect(prepareTargetBatch(allocated, await snapshotBatchTarget(allocated, read), manifest, 'config', 'run', prepare))
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
    expect((await prepareCsv(a7, 'SchemaProfile,ecaas_input_fhs\n')).schema).toBe('"target schema"');
    // Test data alongside the reference makes it an FHS model.
    const withTestData = hp.replace('""product_reference""', '""test_data_EN14825"":[],""product_reference""');
    const { output, schema } = await prepareCsv(a9, withTestData);
    expect(schema).toBe('"target schema"');
    expect(output.validation.is_valid).toBe(true);
  });
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
    await expect(prepareTargetBatch(config, await snapshotBatchTarget(config, read), manifest, 'config', 'run', prepare))
      .rejects.toThrow("one: Internal Gains can't run with HEM model versions yet. Open this Batch Config in Scenarios, uncheck it and save it again.");
    expect(prepare).not.toHaveBeenCalled();
  } finally { assets.mockRestore(); }
});

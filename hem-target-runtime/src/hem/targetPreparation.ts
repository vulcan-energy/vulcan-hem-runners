// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { cloneForHemValidationJson } from '../lib/hemValidationModel';
import { defaultsReadPathAttempts } from '@vulcan-community/geometry-editor/lib/workspacePaths';
import Papa from 'papaparse';
import { getBatchScenarioEntries } from '@repo/core';
import { resolveBatchTarget, resolveHemTarget, targetPreparationContract, targetScenarioModes, type HemTargetManifest } from './targets';
import { loadTargetManifest, sha256, TargetAssets } from './targetAssets';
import type { PreparedHemJob, PreparedWeather } from './preparedRun';
import { allocatedZones } from '../lib/zoneAllocations';
import { TARGET_UNSUPPORTED_CATEGORIES } from './targetCategories';
import { formatCategoryName } from '../utils/textFormatting';
import {
  ecaasOnlyProductLabels,
  ecaasOnlyProductMessage,
  findEcaasOnlyProductsInCsv,
} from '@vulcan-community/geometry-editor/geometry/validation/ecaasOnlyProducts';
import { parseCsvSections } from '@vulcan-community/geometry-editor/geometry/io/csvSectionRows';

/**
 * ECaaS input schema per HEM version (bundle ids are content-addressed, so keying by
 * version means vendoring a schema later never re-pins models). Add a8/a9 here once vendored.
 */
const ECAAS_SCHEMAS: Record<string, () => Promise<string>> = {
  '1.0.0a7': () => import('../../../schemas/ecaas_input_fhs.schema.json?raw').then(module => module.default),
};

export interface TargetBatchSnapshot {
  targetBundleId: string;
  sources: Record<string, { csv: string; sha256: string; defaults?: { json: string; sha256: string } }>;
  snippets: Record<string, { json: string; sha256: string }>;
  weather?: Record<string, PreparedWeather & { selection?: { json: string; sha256: string } }>;
}
function csvMetadata(csv: string, key: string): string | undefined {
  const rows = Papa.parse<string[]>(csv, { skipEmptyLines: true }).data;
  const values: string[] = [];
  for (const row of rows) {
    if (row[0]?.replace(/^#\s*/, '').trim() === key) values.push(row[1]?.trim() ?? '');
  }
  if (values.length > 1) throw new Error(`CSV contains conflicting ${key} metadata`);
  return values[0];
}
export function csvTarget(csv: string): string | undefined { return csvMetadata(csv, 'TargetBundleId'); }
export async function snapshotBatchTarget(config: unknown, readFile: (path: string) => Promise<string>): Promise<TargetBatchSnapshot> {
  const sources: TargetBatchSnapshot['sources'] = {};
  const ids: Array<string | undefined> = [];
  const snippets: TargetBatchSnapshot['snippets'] = {};
  const weather: NonNullable<TargetBatchSnapshot['weather']> = {};
  for (const { value } of getBatchScenarioEntries(config)) {
    const weatherChoice = selectedWeather(value);
    // Preserve configuration order so the first missing/invalid saved input is deterministic.
    // react-doctor-disable-next-line react-doctor/async-await-in-loop -- weather capture is ordered with scenario validation.
    if (weatherChoice && !weather[weatherChoice]) weather[weatherChoice] = await captureWeather(weatherChoice, readFile);
    for (const [category, choices] of Object.entries(value)) {
      if (category === 'base_json' || category === 'zone_allocations' || category === 'model_wrappers' || category === 'weather_files' || category === 'weather_file' || (category === 'external_conditions' && weatherChoice)) continue;
      if (!/^[a-z_]+$/.test(category) || !Array.isArray(choices)) throw new Error('Invalid snippet category');
      for (const choice of choices) {
        if (typeof choice !== 'string' || !/^[a-zA-Z0-9_. -]+$/.test(choice) || choice === '..') throw new Error('Invalid snippet name');
        const path = `input/batch_parameters/${category}/${choice}.json`;
        if (!snippets[path]) {
          // Preserve snippet choice order and surface the first invalid workspace path deterministically.
          // react-doctor-disable-next-line react-doctor/async-await-in-loop -- byte capture follows configuration order.
          const json = await readFile(path);
          JSON.parse(json);
          snippets[path] = { json, sha256: await sha256(new TextEncoder().encode(json)) };
        }
      }
    }
    if (!Array.isArray(value.base_json) || !value.base_json.length) throw new Error('Every scenario needs a source model');
    for (const name of value.base_json) {
      if (typeof name !== 'string' || /[\\/]/.test(name) || name === '..') throw new Error('Invalid base model name');
      if (sources[name]) continue;
      // Validate source models in declared order; defaults capture depends on CSV metadata.
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- preserving order keeps errors and snapshots deterministic.
      const csv = await readFile(`input/base_models/${name}.csv`);
      ids.push(csvTarget(csv));
      sources[name] = { csv, sha256: await sha256(new TextEncoder().encode(csv)), defaults: await captureDeclaredDefaults(csv, readFile) };
    }
  }
  return { targetBundleId: resolveBatchTarget(ids).id, sources, snippets, ...(Object.keys(weather).length ? { weather } : {}) };
}
function unreplacedEcaasOnlyProducts(csv: string, replaced: readonly string[]): string[] {
  if (!replaced.length) return findEcaasOnlyProductsInCsv(csv);
  return parseCsvSections(csv).flatMap((section) => section.rows.flatMap((row) => {
    const cell = row.data.extra_json;
    if (!cell?.includes('product_reference')) return [];
    try {
      const extra = JSON.parse(cell) as Record<string, unknown>;
      return ecaasOnlyProductLabels(Object.fromEntries(Object.entries(extra).filter(([key]) => !replaced.includes(key))));
    } catch {
      return []; // malformed extra_json is the merge's error to report
    }
  }));
}
/** Snippet categories that replace a whole top-level section, product references included. */
const REPLACED_SECTIONS: Record<string, string> = { heat_source_wet: 'HeatSourceWet', hot_water_source: 'HotWaterSource' };
/**
 * HEM cannot resolve ECaaS product references locally. Returns the stop message when a
 * scenario runs an ECaaS-only product its snippets don't replace.
 */
export function ecaasOnlyBatchMessage(config: unknown, snapshot: TargetBatchSnapshot): string | undefined {
  const labels = new Set<string>();
  for (const { value } of getBatchScenarioEntries(config)) {
    const replaced = Object.entries(REPLACED_SECTIONS).flatMap(([category, section]) =>
      Array.isArray(value[category]) && (value[category] as unknown[]).length ? [section] : []);
    for (const name of Array.isArray(value.base_json) ? value.base_json : []) {
      const source = snapshot.sources[name as string];
      if (source) unreplacedEcaasOnlyProducts(source.csv, replaced).forEach((label) => labels.add(label));
    }
  }
  return labels.size ? ecaasOnlyProductMessage([...labels]) : undefined;
}
export type PrepareScenario = (request: string, snippets: string) => string;
const preparations = new Map<string, Promise<PrepareScenario>>();
/**
 * One preparation instance per target in this context: instantiating the module costs more
 * than a preparation, and the instance keeps its compiled-schema cache. A throw (a Rust
 * panic traps the instance) drops it so the next call starts clean.
 */
export function loadTargetPreparation(manifest: HemTargetManifest): Promise<PrepareScenario> {
  const id = manifest.bundleId;
  let loaded = preparations.get(id);
  if (!loaded) {
    loaded = instantiateTargetPreparation(manifest);
    preparations.set(id, loaded);
    loaded.catch(() => preparations.delete(id));
  }
  return loaded.then(prepare => (request, snippets) => {
    try { return prepare(request, snippets); } catch (error) { preparations.delete(id); throw error; }
  });
}
async function instantiateTargetPreparation(manifest: HemTargetManifest): Promise<PrepareScenario> {
  const assets = await TargetAssets.open();
  const artifact = (path: string) => {
    const artifact = manifest.artifacts.find(asset => asset.path === path);
    if (!artifact) throw new Error(`Missing preparation asset ${path}`);
    return artifact;
  };
  const [javascript, wasm] = await Promise.all([
    assets.bytes(artifact(manifest.preparation.javascript)),
    assets.bytes(artifact(manifest.preparation.wasm)),
  ]);
  const url = URL.createObjectURL(new Blob([javascript as Uint8Array<ArrayBuffer>], { type: 'text/javascript' }));
  try {
    // Runtime code is loaded from the selected content-addressed artifact, not the app bundle.
    // react-doctor-disable-next-line react-doctor/no-dynamic-import-path -- the verified target JavaScript is intentionally imported from a blob URL.
    const module = await import(/* @vite-ignore */ url);
    await module.default({ module_or_path: wasm });
    if (typeof module.prepare_target_scenario !== 'function') throw new Error('Pinned preparation artifact does not implement target preparation protocol 1');
    return module.prepare_target_scenario;
  } finally { URL.revokeObjectURL(url); }
}
export async function prepareTargetBatch(
  config: Record<string, unknown>, snapshot: TargetBatchSnapshot, manifest: HemTargetManifest,
  configName: string, runId: string, prepare: PrepareScenario,
): Promise<PreparedHemJob[]> {
  const target = resolveHemTarget(snapshot.targetBundleId);
  if (manifest.bundleId !== target.id) throw new Error('Batch and preparation target differ');
  const assets = await TargetAssets.open();
  const text = async (path: string) => {
    const artifact = manifest.artifacts.find(artifact => artifact.path === path);
    if (!artifact) throw new Error(`Missing preparation content ${path}`);
    return new TextDecoder().decode(await assets.bytes(artifact));
  };
  const [schema_json, defaults_json] = await Promise.all([
    text(manifest.preparation.schema),
    text(manifest.preparation.defaults),
  ]);
  const jobs: PreparedHemJob[] = [];
  for (const { name, value } of getBatchScenarioEntries(config)) {
    const weatherChoice = selectedWeather(value);
    const weather = weatherChoice ? snapshot.weather?.[weatherChoice] : undefined;
    if (weatherChoice) {
      if (!weather) throw new Error('Missing or changed saved weather. Refresh and resave the batch.');
      // Keep integrity checks in scenario order so failures are deterministic before any execution starts.
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- each scenario is validated before preparing the next.
      if (await sha256(new TextEncoder().encode(weather.epw)) !== weather.sha256) throw new Error('Missing or changed saved weather. Refresh and resave the batch.');
      assertWeatherFilename(weather.path);
      if (weather.selection) {
        if (await sha256(new TextEncoder().encode(weather.selection.json)) !== weather.selection.sha256 || JSON.parse(weather.selection.json).weather_file !== weather.path) throw new Error('Changed saved weather selection. Refresh and resave the batch.');
      } else if (weatherChoice !== weather.path) throw new Error('Saved weather selection identity mismatch');
    }
    // Configs saved before empty zone entries were pruned still carry {"Zone 1": {}}.
    if (allocatedZones(value.zone_allocations as Record<string, Record<string, unknown>> | undefined)) throw new Error('This Batch Config was saved with per-zone selections. Open it in Scenarios and save it again.');
    const entries = Object.entries(value).filter(([key]) => key !== 'zone_allocations');
    for (const [category, choices] of entries) {
      if (TARGET_UNSUPPORTED_CATEGORIES.has(category) && Array.isArray(choices) && choices.length > 0) {
        throw new Error(`${name}: ${formatCategoryName(category)} can't run with HEM model versions yet. Open this Batch Config in Scenarios, uncheck it and save it again.`);
      }
    }
    let combinations: Array<Array<[string, string]>> = [[]];
    for (const [category, choices] of entries) {
      if (!Array.isArray(choices) || !choices.length || choices.some(choice => typeof choice !== 'string')) throw new Error(`Invalid scenario choices for ${category}`);
      combinations = combinations.flatMap(combination => choices.map(choice => [...combination, [category, choice] as [string, string]]));
    }
    for (const [index, combination] of combinations.entries()) {
      const choicesByCategory = new Map(combination);
      const sourceName = choicesByCategory.get('base_json');
      const source = sourceName ? snapshot.sources[sourceName] : undefined;
      if (!source || csvTarget(source.csv) !== target.id) throw new Error('Saved batch source snapshot identity/hash mismatch; explicitly refresh and resave it.');
      // Keep source and snippet integrity failures ordered before preparation starts.
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- jobs are validated in their saved Cartesian-product order.
      if (await sha256(new TextEncoder().encode(source.csv)) !== source.sha256) throw new Error('Saved batch source snapshot identity/hash mismatch; explicitly refresh and resave it.');
      const snippets: Array<[string, unknown]> = [];
      let modes = ['actual'];
      for (const [category, choice] of combination) {
        if (category === 'base_json' || category === 'weather_files' || category === 'weather_file' || (category === 'external_conditions' && weather)) continue;
        if (category === 'model_wrappers') {
          modes = targetScenarioModes(target, choice);
          continue;
        }
        if (!/^[a-z_]+$/.test(category) || !/^[a-zA-Z0-9_. -]+$/.test(choice) || choice === '..') throw new Error('Invalid snippet path');
        const path = `input/batch_parameters/${category}/${choice}.json`;
        const captured = snapshot.snippets[path];
        if (!captured) throw new Error(`Missing or changed saved snippet ${path}. Refresh and resave the batch.`);
        // Preserve snippet-order validation and fail before invoking the preparation runtime.
        // react-doctor-disable-next-line react-doctor/async-await-in-loop -- same ordered preparation contract as the saved combinations.
        if (await sha256(new TextEncoder().encode(captured.json)) !== captured.sha256) throw new Error(`Missing or changed saved snippet ${path}. Refresh and resave the batch.`);
        snippets.push([category, JSON.parse(captured.json)]);
      }
      await assertTargetDefaults(source.csv, source.defaults, defaults_json);
      const request = { csv: source.csv, schema_json, defaults_json, ...targetPreparationContract(target) };
      const result = JSON.parse(prepare(JSON.stringify(request), JSON.stringify(snippets)));
      if (!result.ok) throw new Error(result.error ?? 'Target preparation failed');
      if (!result.output.validation.is_valid) throw new Error(`Model ${sourceName}: ${result.output.validation.errors.map((error: {message?: string; user_message?: string}) => error.user_message ?? error.message ?? JSON.stringify(error)).join('; ')}`);
      // Non-blocking scenario diagnostics (e.g. a stale a9 whole-wall U) travel with the run record.
      const warnings = [...new Set<string>((result.output.schema_omissions ?? []).filter((w: {code?: string}) => w.code === 'W_TARGET_INPUT').map((w: {message: string}) => w.message))];
      jobs.push({ name: `${name}_${index}`, ...(warnings.length ? { warnings } : {}), input: cloneForHemValidationJson(JSON.stringify(result.output.model)), modes, sourceCsv: source.csv, ...(weather ? { weather: { path: weather.path, epw: weather.epw, sha256: weather.sha256 } } : {}),
        outputDirectory: `output/result_${configName}_${name}_${index}`, archiveDirectory: `output/result_${configName}_${name}_${index}/${target.id}/${runId}` });
    }
  }
  return jobs;
}

/** Standalone merge uses the same preparation export as changed scenarios. */
export async function prepareTargetCsv(csv: string, readFile?: (path: string) => Promise<string>) {
  const id = csvTarget(csv);
  if (!id) throw new Error('Select a target before preparing this model');
  const manifest = await loadTargetManifest(id);
  return prepareTargetCsvWithManifest(csv, manifest, readFile);
}

/** Live preflight: prepares only when this target's preparation files are already downloaded. */
export async function prepareTargetCsvIfCached(csv: string, readFile?: (path: string) => Promise<string>) {
  const id = csvTarget(csv);
  if (!id) return undefined;
  const manifest = await loadTargetManifest(id);
  const { missing } = await (await TargetAssets.open()).inspect(preparationManifest(manifest));
  return missing.length ? undefined : prepareTargetCsvWithManifest(csv, manifest, readFile);
}
function preparationManifest(manifest: HemTargetManifest): HemTargetManifest {
  const paths = new Set(Object.values(manifest.preparation));
  return { ...manifest, artifacts: manifest.artifacts.filter(artifact => paths.has(artifact.path)) };
}

/** Explicit local tools verify the manifest digest before calling this same preparation boundary. */
export async function prepareTargetCsvWithManifest(csv: string, manifest: HemTargetManifest, readFile?: (path: string) => Promise<string>) {
  const id = csvTarget(csv);
  if (!id || id !== manifest.bundleId) throw new Error('Source CSV and preparation target differ');
  const target = resolveHemTarget(id);
  const assets = await TargetAssets.open();
  await assets.download(preparationManifest(manifest), () => {});
  const text = async (path: string) => new TextDecoder().decode(await assets.bytes(manifest.artifacts.find(artifact => artifact.path === path)!));
  // The product decides, not the SchemaProfile marker (stale _pcdb metadata, or CSVs authored elsewhere).
  // Test data entered alongside a reference (Advanced Fields) makes the model a plain FHS model.
  const ecaas = findEcaasOnlyProductsInCsv(csv).length > 0;
  const ecaasSchema = ecaas ? ECAAS_SCHEMAS[target.wrapperVersion] : undefined;
  const unavailable = ecaas && !ecaasSchema;
  const [defaults_json, schema_json, capturedDefaults, prepare] = await Promise.all([
    text(manifest.preparation.defaults),
    ecaasSchema ? ecaasSchema() : text(manifest.preparation.schema),
    captureDeclaredDefaults(csv, readFile),
    loadTargetPreparation(manifest),
  ]);
  await assertTargetDefaults(csv, capturedDefaults, defaults_json);
  const result = JSON.parse(prepare(JSON.stringify({ csv, schema_json,
    defaults_json, ...targetPreparationContract(target) }), '[]'));
  if (!result.ok) throw new Error(result.error ?? 'Target conversion failed');
  // Provisional, not a verdict: the next save validates once ECAAS_SCHEMAS gains this version.
  const validation = unavailable ? { is_valid: false, errors: [...result.output.validation.errors, { code: 'ecaas_schema_unavailable', path: '',
    message: `ECaaS-only product: there is no ECaaS schema for HEM ${target.version} yet. Pin this model to HEM ${Object.keys(ECAAS_SCHEMAS).join(' or ')} to validate and submit it to ECaaS.` }] } : result.output.validation;
  return { ...result.output, validation, targetBundleId: target.id };
}

async function captureDeclaredDefaults(csv: string, readFile?: (path: string) => Promise<string>) {
  const path = csvMetadata(csv, 'DefaultsPath');
  if (!path) return undefined;
  if (!readFile) throw new Error(`Read the defaults declared by this source before preparing it: ${path}`);
  let json: string | undefined;
  for (const candidate of defaultsReadPathAttempts(path)) {
    // Aliases are tried in their established precedence; stop at the first readable file.
    // react-doctor-disable-next-line react-doctor/async-await-in-loop -- fallback order is part of persisted legacy path resolution.
    try { json = await readFile(candidate); break; } catch { /* Try only established legacy path aliases. */ }
  }
  if (json === undefined) throw new Error(`Cannot read declared defaults '${path}'. Restore the file before preparing this source.`);
  JSON.parse(json);
  return { json, sha256: await sha256(new TextEncoder().encode(json)) };
}
async function assertTargetDefaults(csv: string, captured: { json: string; sha256: string } | undefined, pinned: string): Promise<void> {
  const path = csvMetadata(csv, 'DefaultsPath');
  if (!path) return;
  if (!captured || await sha256(new TextEncoder().encode(captured.json)) !== captured.sha256) throw new Error(`Missing or changed saved defaults '${path}'. Explicitly refresh and resave the batch.`);
  if (captured.sha256 !== await sha256(new TextEncoder().encode(pinned))) {
    throw new Error(`Defaults '${path}' differ from this target's pinned defaults. If it is the stock template, update the sample library to refresh it; otherwise preserve this file and move compatible authored settings into CSV or target-compatible snippets before using the pinned defaults.`);
  }
}

function selectedWeather(scenario: Record<string, unknown>): string | undefined {
  if ('weather_file' in scenario && 'weather_files' in scenario) throw new Error('Use weather_files or weather_file, not both');
  const choices = scenario.weather_files ?? scenario.weather_file;
  if (choices === undefined) return undefined;
  if (!Array.isArray(choices) || choices.length !== 1 || typeof choices[0] !== 'string') throw new Error('Select exactly one weather file per scenario');
  if (!choices[0]) return undefined; // Existing empty selection means pinned reference weather.
  if (!/^[a-zA-Z0-9_. -]+$/.test(choices[0]) || choices[0] === '..') throw new Error('Invalid weather selection');
  return choices[0];
}
function assertWeatherFilename(filename: unknown): asserts filename is string {
  if (typeof filename !== 'string' || !/^[a-zA-Z0-9_. -]+\.epw$/.test(filename)) throw new Error('Invalid weather filename: select an EPW inside batch_parameters/weather_files');
}
async function captureWeather(choice: string, readFile: (path: string) => Promise<string>) {
  let path = choice;
  let selection: { json: string; sha256: string } | undefined;
  if (!choice.endsWith('.epw')) {
    const json = await readFile(`input/batch_parameters/weather_files/${choice}.json`);
    path = JSON.parse(json).weather_file;
    selection = { json, sha256: await sha256(new TextEncoder().encode(json)) };
  }
  assertWeatherFilename(path);
  const epw = await readFile(`input/batch_parameters/weather_files/${path}`);
  if (!epw.trim()) throw new Error(`Weather file ${path} is empty`);
  return { path, epw, sha256: await sha256(new TextEncoder().encode(epw)), ...(selection ? { selection } : {}) };
}

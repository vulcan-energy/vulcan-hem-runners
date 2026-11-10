// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { cloneForHemValidationJson } from '../lib/hemValidationModel';
import { defaultsReadPathAttempts } from '@vulcan-community/geometry-editor/lib/workspacePaths';
import { EXTRA_JSON_UI_KEYS } from '@vulcan-community/geometry-editor/lib/csvPresetUtils';
import { targetFieldAvailability } from '@vulcan-community/geometry-editor/lib/schemaCache';
import { getBatchScenarioEntries } from '@repo/core';
import { resolveBatchTarget, resolveHemTarget, targetPreparationContract, targetScenarioModes, type HemTarget, type HemTargetManifest } from './targets';
import { loadTargetManifest, sha256, TargetAssets } from './targetAssets';
import { correctTargetSchema } from './targetSchemaCorrections';
import type { PreparedHemJob, PreparedWeather } from './preparedRun';
import { allocatedZones, savedScenarioSelections } from '../lib/zoneAllocations';
import { parseHeatingProfile, type HeatingProfile } from './heatingProfile';
import { SAVED_SELECTION_REMEDY, savedSelectionErrors, TARGET_UNSUPPORTED_CATEGORIES } from './targetCategories';
import { formatCategoryName } from '../utils/textFormatting';
import {
  ecaasOnlyProductLabels,
  ecaasOnlyProductMessage,
  findEcaasOnlyProductsInCsv,
} from '@vulcan-community/geometry-editor/geometry/validation/ecaasOnlyProducts';
import { csvMetadataValue as csvMetadata, parseCsvSections } from '@vulcan-community/geometry-editor/geometry/io/csvSectionRows';
import { parseCsvToGeometry } from '@vulcan-community/geometry-editor/geometry/io/parseCsvToGeometry';
import { validateSpaceLabels } from '@vulcan-community/geometry-editor/geometry/validation/validateSpaceLabels';
import { editorFloorsAndElementsForParsedCsv } from '@vulcan-community/geometry-editor/lib/floorDerivation';

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
export function csvTarget(csv: string): string | undefined { return csvMetadata(csv, 'TargetBundleId'); }
export async function snapshotBatchTarget(config: unknown, readFile: (path: string) => Promise<string>): Promise<TargetBatchSnapshot> {
  const sources: TargetBatchSnapshot['sources'] = {};
  const ids: Array<string | undefined> = [];
  const snippets: TargetBatchSnapshot['snippets'] = {};
  const weather: NonNullable<TargetBatchSnapshot['weather']> = {};
  // The save-time check the Scenarios page runs; MCP and Analyst save through this snapshot.
  const selectionErrors = getBatchScenarioEntries(config).flatMap(({ name, value }) => savedSelectionErrors({ name, ...savedScenarioSelections(value) }));
  if (selectionErrors.length) throw new Error(selectionErrors.join('\n'));
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
function treatedFloorAreaErrors(csv: string): string[] {
  // Without labels there is nothing to check, and the strict parser must not gate such a source.
  if (!/^#?Space Labels\b/m.test(csv)) return [];
  const parsed = parseCsvToGeometry(csv);
  const { elements } = editorFloorsAndElementsForParsedCsv(parsed);
  return validateSpaceLabels(
    Object.fromEntries(parsed.spaceLabels.map((label) => [label.id, label])),
    parsed.spaceLabels.map((label) => label.id),
    {
      zones: parsed.zones,
      elementsById: Object.fromEntries(elements.map((element) => [element.id, element])),
      // As the editor: the FHS check applies whatever ComplianceValidationEnabled says.
      complianceValidationEnabled: true,
    },
  ).issues.map((issue) => issue.message);
}
/** Stop message when a model that would run has a materially wrong treated floor area. */
export function treatedFloorAreaBatchMessage(config: unknown, snapshot: TargetBatchSnapshot): string | undefined {
  const names = new Set(getBatchScenarioEntries(config).flatMap(({ value }) =>
    Array.isArray(value.base_json) ? value.base_json as string[] : []));
  const messages = [...names].flatMap((name) => {
    const source = snapshot.sources[name];
    return source ? treatedFloorAreaErrors(source.csv).map((message) => `${name}: ${message}`) : [];
  });
  return messages.length ? messages.join('\n') : undefined;
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
  configName: string, runId: string, startedAt: string, prepare: PrepareScenario,
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
    text(manifest.preparation.schema).then(schema => correctTargetSchema(target.id, schema)),
    text(manifest.preparation.defaults),
  ]);
  const targetSchema = JSON.parse(schema_json) as { $defs?: { Tank?: { properties?: Record<string, unknown> } } };
  const tank = targetSchema.$defs?.Tank;
  // CSV preparation drops a cylinder init_temp the target does not declare; snippets match that.
  const omitsCylinderInitTemp = !!tank && !tank.properties?.init_temp;
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
        throw new Error(`${name}: ${formatCategoryName(category)} can't run with HEM model versions yet — ${SAVED_SELECTION_REMEDY}.`);
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
      const parameterFiles: string[] = [];
      const omittedWarnings: string[] = [];
      let modes = ['actual'];
      let heatingProfile: HeatingProfile | undefined;
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
        if (category === 'heating_profiles') heatingProfile = parseHeatingProfile(JSON.parse(captured.json));
        else {
          const snippet = JSON.parse(captured.json);
          const cylinder = snippet?.HotWaterSource?.['hw cylinder'];
          if (category === 'hot_water_source' && omitsCylinderInitTemp && cylinder && 'init_temp' in cylinder) {
            delete cylinder.init_temp;
            omittedWarnings.push(`HEM ${target.version}: the cylinder init_temp in this hot water snippet is not used. It was omitted from the run.`);
          }
          // Compliance snippets write root keys; a count the target's schema lacks is not an input there.
          if (category === 'compliance_settings') {
            const absent = Object.keys(snippet).filter(key => targetFieldAvailability(targetSchema, [key]) === 'absent');
            absent.forEach(key => delete snippet[key]);
            if (absent.length) omittedWarnings.push(`HEM ${target.version}: ${absent.join(', ')} in this compliance snippet is not used. It was omitted from the run.`);
          }
          snippets.push([category, snippet]);
          parameterFiles.push(`${category}/${choice}.json`);
        }
      }
      if (heatingProfile && (modes.length !== 1 || modes[0] !== 'actual')) throw new Error('Heating profiles require actual alone; they are not FHS compliance calculations');
      await assertTargetDefaults(source.csv, source.defaults, defaults_json);
      const request = { csv: source.csv, schema_json, defaults_json, ...targetPreparationContract(target) };
      const result = JSON.parse(prepare(JSON.stringify(request), JSON.stringify(snippets)));
      if (!result.ok) throw new Error(result.error ?? 'Target preparation failed');
      const snippetKeys = new Set(snippets.filter(([category]) => category === 'compliance_settings').flatMap(([, snippet]) => Object.keys(snippet as object)));
      result.output = withFhsInputErrors(withPythonFanPositionErrors(result.output, target), source.csv, snippetKeys);
      if (!result.output.validation.is_valid) {
        // The model prepared alone, to tell its own errors from the ones that appear only with this scenario's files.
        const base = JSON.parse(prepare(JSON.stringify(request), '[]'));
        if (!base.ok) throw new Error(base.error ?? 'Target preparation failed');
        const modelErrors = withFhsInputErrors(withPythonFanPositionErrors(base.output, target), source.csv).validation.errors;
        throw new Error(`Scenario ${name}_${index}. ${scenarioValidationMessage(result.output.validation.errors, modelErrors, sourceName!, parameterFiles)}`);
      }
      // Non-blocking scenario diagnostics (e.g. a stale a9 whole-wall U) travel with the run record.
      const authoredOmission = authoredOmissionWarning(result.output.schema_omissions, source.csv, target.version);
      const warnings = [...new Set<string>([...omittedWarnings, ...(authoredOmission ? [authoredOmission] : []), ...(result.output.schema_omissions ?? []).filter((w: {code?: string}) => w.code === 'W_TARGET_INPUT').map((w: {message: string}) => w.message)])];
      jobs.push({ startedAt, ...(heatingProfile ? { heatingProfile } : {}), name: `${name}_${index}`, ...(warnings.length ? { warnings } : {}), input: cloneForHemValidationJson(JSON.stringify(result.output.model)), modes, sourceCsv: source.csv, ...(weather ? { weather: { path: weather.path, epw: weather.epw, sha256: weather.sha256 } } : {}),
        ...(parameterFiles.length ? { parameterFiles } : {}), outputDirectory: `output/result_${configName}_${name}_${index}`, archiveDirectory: `output/result_${configName}_${name}_${index}/${target.id}/${runId}` });
    }
  }
  return jobs;
}

type ValidationError = { path?: string; code?: string; keyword?: string; message?: string; user_message?: string };
/**
 * Validation errors grouped by what was observed: whether the model prepared alone also fails the same rule
 * at the same place, or the error occurs only when the scenario's parameter files are applied. Which of those
 * files causes it is not known, as several can set the same section.
 */
function scenarioValidationMessage(errors: ValidationError[], modelErrors: ValidationError[], model: string, parameterFiles: string[]): string {
  // Messages quote the failing object, which the scenario's files can change, so errors are compared by rule and place.
  // ponytail: two different errors of the same kind at the same path compare equal.
  const key = (error: ValidationError) => [error.path, error.code, error.keyword].join('\n');
  const own = new Set(modelErrors.map(key));
  const describe = (list: ValidationError[]) => list.map(error => {
    const text = error.user_message ?? error.message ?? JSON.stringify(error);
    return error.path ? `${text} at ${error.path}` : text;
  }).join('; ');
  const inModel = errors.filter(error => own.has(key(error)));
  const scenarioOnly = errors.filter(error => !own.has(key(error)));
  return [
    ...inModel.length ? [`The model "${model}" has ${inModel.length === 1 ? 'this error' : 'these errors'} without the scenario's parameter files as well: ${describe(inModel)}.`] : [],
    ...scenarioOnly.length ? [`${scenarioOnly.length === 1 ? 'This error occurs' : 'These errors occur'} only with the scenario's parameter files applied (${parameterFiles.join(', ')}): ${describe(scenarioOnly)}.`] : [],
  ].join(' ');
}

const PYTHON_FLAT_POSITION_VENT_TYPES = new Set(['Intermittent MEV', 'Centralised continuous MEV', 'Decentralised continuous MEV']);

/**
 * The schemas accept `position_exhaust` on these fans, but Python HEM reads only the flat fields and
 * fails with a raw KeyError. Report it as a required field before the run.
 */
export function withPythonFanPositionErrors<T extends { model?: unknown; validation: { is_valid: boolean; errors: unknown[] } }>(output: T, target: HemTarget): T {
  if (target.runtime !== 'python') return output;
  const fans = (output.model as { InfiltrationVentilation?: { MechanicalVentilation?: Record<string, Record<string, unknown>> } } | undefined)
    ?.InfiltrationVentilation?.MechanicalVentilation ?? {};
  const errors = Object.entries(fans)
    .filter(([, fan]) => PYTHON_FLAT_POSITION_VENT_TYPES.has(String(fan.vent_type)) && fan.orientation360 === undefined)
    .map(([name]) => ({
      code: 'E026',
      keyword: 'required',
      path: `/InfiltrationVentilation/MechanicalVentilation/${name.replace(/~/g, '~0').replace(/\//g, '~1')}/orientation360`,
      message: `'orientation360' is a required property`,
      user_message: `Fan "${name}": this HEM version needs height, orientation and pitch as separate fields. In Advanced, set Exhaust position to Flat fields.`,
    }));
  return errors.length ? { ...output, validation: { ...output.validation, is_valid: false, errors: [...output.validation.errors, ...errors] } } : output;
}

/** Element keys the editor keeps for itself; dropping them from a run costs the user nothing. */
const EDITOR_ONLY_KEYS = new Set<string>([...EXTRA_JSON_UI_KEYS, 'floor_id', 'geometry_face', 'party_wall_lining_type', 'dormer_bundle', 'parent_netting_area']);

/**
 * One line naming the values this source set that the target dropped. A template value never
 * appears in the CSV, which is how it is told apart from an authored one.
 */
export function authoredOmissionWarning(omissions: Array<{ code?: string; path: string }> | undefined, csv: string, version: string): string | undefined {
  const fields = [...new Set((omissions ?? []).filter(omission => omission.code === 'TARGET_FIELD_OMITTED').map(omission => omission.path.slice(omission.path.lastIndexOf('/') + 1).replace(/~1/g, '/').replace(/~0/g, '~')))]
    .filter(field => !field.startsWith('_') && !EDITOR_ONLY_KEYS.has(field) && csv.includes(field));
  return fields.length ? `HEM ${version} does not use ${fields.join(', ')} from this model. Omitted from the run; the source keeps ${fields.length > 1 ? 'them' : 'it'}.` : undefined;
}

const ROOM_COUNTS_TAKEN_FROM_TEMPLATE = ['NumberOfBedrooms', 'NumberOfWetRooms'];

/**
 * Inputs FHS crashes on, and counts the template would silently fill with 1. Reported as
 * ordinary validation errors before the run. `snippetKeys` are root keys a scenario authored.
 */
export function withFhsInputErrors<T extends { model?: unknown; validation: { is_valid: boolean; errors: unknown[] } }>(output: T, csv: string, snippetKeys: ReadonlySet<string> = new Set()): T {
  const model = (output.model ?? {}) as Record<string, unknown> & { HotWaterDemand?: { Shower?: object; Bath?: object } };
  const demand = model.HotWaterDemand;
  const errors: unknown[] = [];
  if (demand && !Object.keys(demand.Shower ?? {}).length && !Object.keys(demand.Bath ?? {}).length) {
    errors.push({ code: 'E_FHS_NO_SHOWER_OR_BATH', keyword: 'required', path: '/HotWaterDemand/Shower', message: 'FHS needs a shower or a bath', user_message: 'FHS cannot run a dwelling with no shower and no bath. Add at least one in Hot Water Demand.' });
  }
  for (const key of ROOM_COUNTS_TAKEN_FROM_TEMPLATE) {
    if (model[key] !== undefined && !csvMetadata(csv, key) && !snippetKeys.has(key)) {
      errors.push({ code: 'E_FHS_MISSING_ROOM_COUNT', keyword: 'required', path: `/${key}`, message: `'${key}' is a required property`, user_message: `${key} is missing from this model, so the template value of 1 would be used. Enter it in Global Settings.` });
    }
  }
  return errors.length ? { ...output, validation: { ...output.validation, is_valid: false, errors: [...output.validation.errors, ...errors] } } : output;
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
    ecaasSchema ? ecaasSchema() : text(manifest.preparation.schema).then(schema => correctTargetSchema(target.id, schema)),
    captureDeclaredDefaults(csv, readFile),
    loadTargetPreparation(manifest),
  ]);
  await assertTargetDefaults(csv, capturedDefaults, defaults_json);
  const result = JSON.parse(prepare(JSON.stringify({ csv, schema_json,
    defaults_json, ...targetPreparationContract(target) }), '[]'));
  if (!result.ok) throw new Error(result.error ?? 'Target conversion failed');
  result.output = withFhsInputErrors(withPythonFanPositionErrors(result.output, target), csv);
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

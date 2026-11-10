// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

/** Effective high-temperature hours, after FHS advanced start. Times are hours from midnight. */
export type HeatingProfile = { type: 'reference' } | {
  type: 'weekly'; weekday: [number, number][]; weekend: [number, number][]; setback: number;
};
export function parseHeatingProfile(value: unknown): HeatingProfile {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Heating profile must be an object');
  const profile = value as Record<string, unknown>;
  const keys = profile.type === 'reference' ? ['type'] : ['type', 'weekday', 'weekend', 'setback'];
  if (Object.keys(profile).some(key => !keys.includes(key))) throw new Error('Unknown heating profile field');
  if (profile.type === 'reference') return { type: 'reference' };
  if (profile.type !== 'weekly') throw new Error('Heating profile type must be reference or weekly');
  if (typeof profile.setback !== 'number' || !Number.isFinite(profile.setback) || profile.setback < 0 || profile.setback > 30) throw new Error('Heating setback must be between 0 and 30 C');
  for (const day of ['weekday', 'weekend']) {
    const periods = profile[day];
    if (!Array.isArray(periods) || !periods.length) throw new Error(`Heating ${day} needs at least one period`);
    let end = 0;
    for (const period of periods) {
      if (!Array.isArray(period) || period.length !== 2 || period.some(time => typeof time !== 'number' || !Number.isFinite(time)) || period[0] < 0 || period[1] > 24 || period[0] >= period[1]) throw new Error('Heating periods must be [start, end] within 0–24 hours');
      if (period[0] < end) throw new Error('Heating periods must be ordered without overlap');
      end = period[1];
    }
  }
  return structuredClone(profile) as HeatingProfile;
}

export function applyHeatingProfile(input: string, supplied: HeatingProfile): string {
  const profile = parseHeatingProfile(supplied);
  if (profile.type === 'reference') return input;
  const model = JSON.parse(input);
  const names = new Set<string>();
  for (const system of Object.values(model.SpaceHeatSystem ?? {}) as Array<{Control?: string}>) {
    if (typeof system.Control !== 'string') throw new Error('Space heating system has no generated setpoint control');
    names.add(system.Control);
  }
  if (!names.size) throw new Error('No space heating controls to adjust');
  for (const name of names) {
    const control = model.Control?.[name];
    if (control?.type !== 'SetpointTimeControl' || control.start_day !== 0) throw new Error(`Unsupported generated heating control ${name}`);
    const step = control.time_series_step;
    if (!(step > 0) || !Number.isInteger(24 / step)) throw new Error('Heating control timestep must divide a day');
    // Read only schedule values, never repeat counts or other controller parameters.
    const targets = new Set<number>();
    const visit = (entries: unknown, active = new Set<string>()) => {
      if (!Array.isArray(entries)) throw new Error('Unsupported generated heating schedule');
      for (const entry of entries) {
        const value = entry && typeof entry === 'object' ? entry.value : entry;
        if (typeof value === 'number' && Number.isFinite(value)) targets.add(value);
        else if (typeof value === 'string') {
          if (active.has(value)) throw new Error('Cyclic heating schedule');
          visit(control.schedule[value], new Set([...active, value]));
        } else if (value !== null) throw new Error('Unsupported generated heating schedule value');
      }
    };
    visit(control.schedule?.main);
    if (targets.size !== 1) throw new Error(`Heating control ${name} must have a single generated occupied target`);
    const target = [...targets][0];
    if (profile.setback > target) throw new Error('Heating setback exceeds the generated occupied target');
    const day = (periods: [number, number][]) => {
      if (periods.flat().some(hour => !Number.isInteger(hour / step))) throw new Error('Heating periods must align with the generated control timestep');
      return Array.from({ length: 24 / step }, (_, index) => periods.some(([start, end]) => index * step >= start && index * step < end) ? target : null);
    };
    control.schedule = {
      main: [{ value: 'week', repeat: Math.ceil(model.SimulationTime.end / 168) + 1 }],
      week: [{ value: 'weekday', repeat: 5 }, { value: 'weekend', repeat: 2 }],
      weekday: day(profile.weekday), weekend: day(profile.weekend),
    };
    control.advanced_start = 0;
    control.setpoint_min = profile.setback;
  }
  return JSON.stringify(model);
}

export type HeatingStage = (input: string, stage: 'preprocess' | 'core') => Promise<Record<string, string>>;
export async function runHeatingProfile(input: string, profile: HeatingProfile, run: HeatingStage): Promise<Record<string, string>> {
  const exported = await run(input, 'preprocess');
  const names = Object.keys(exported).filter(name => name.endsWith('__preproc.json'));
  if (names.length !== 1) throw new Error('Heating sensitivity requires exactly one actual FHS export');
  const original = exported[names[0]];
  const adjusted = applyHeatingProfile(original, profile);
  const results = await run(adjusted, 'core');
  if (!Object.keys(results).some(name => name.endsWith('__results.csv'))) throw new Error('Core replay produced no timestep results');
  if ('fhs_compliance_report.json' in results) throw new Error('Operating sensitivity cannot return an FHS compliance report');
  return { ...results, 'fhs-preprocessed.json': original, 'heating-input.json': adjusted, 'heating-profile.json': JSON.stringify(profile) };
}

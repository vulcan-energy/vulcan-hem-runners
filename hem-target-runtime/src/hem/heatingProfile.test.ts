// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
import { describe, expect, it } from 'vitest';
import { applyHeatingProfile, parseHeatingProfile, runHeatingProfile } from './heatingProfile';

const model = () => ({
  SimulationTime: { start: 0, end: 8760, step: 0.5 },
  SpaceHeatSystem: { heating: { Control: 'space' } },
  Control: {
    space: { type: 'SetpointTimeControl', start_day: 0, time_series_step: 0.5, advanced_start: 2, setpoint_min: 18,
      schedule: { main: [{ value: 'week', repeat: 53 }], week: [{ value: 'weekday', repeat: 5 }, { value: 'weekend', repeat: 2 }],
        weekday: Array.from({ length: 48 }, (_, i) => i >= 14 && i < 19 || i >= 33 && i < 44 ? 20.4 : null),
        weekend: Array.from({ length: 48 }, (_, i) => i >= 17 && i < 44 ? 20.4 : null) } },
    dhw: { schedule: { main: [52] } },
  },
});
const earlier = { type: 'weekly', weekday: [[4, 8.5], [13.5, 21]], weekend: [[5.5, 21]], setback: 18 };
describe('post-FHS heating controls', () => {
  it('leaves an unchanged replay byte-for-byte intact', () => {
    const input = JSON.stringify(model(), null, 2);
    expect(applyHeatingProfile(input, { type: 'reference' })).toBe(input);
  });
  it('changes only linked heating controls, retains target temperature, and applies advance once', () => {
    const original = model();
    const changed = JSON.parse(applyHeatingProfile(JSON.stringify(original), parseHeatingProfile(earlier)));
    const control = changed.Control.space;
    expect(control.advanced_start).toBe(0);
    expect(control.schedule.weekday.filter((v: unknown) => v === 20.4)).toHaveLength(24);
    expect(control.schedule.weekday[7]).toBeNull();
    expect(control.schedule.weekday[8]).toBe(20.4);
    expect(control.schedule.weekday[17]).toBeNull();
    expect(control.setpoint_min).toBe(18);
    changed.Control.space = original.Control.space;
    expect(changed).toEqual(original);
  });
  it('supports continuous thermostat control and lower setback', () => {
    const full = JSON.parse(applyHeatingProfile(JSON.stringify(model()), parseHeatingProfile({ type: 'weekly', weekday: [[0, 24]], weekend: [[0, 24]], setback: 15 })));
    expect(full.Control.space.schedule.weekday).toEqual(Array(48).fill(20.4));
    expect(full.Control.space.setpoint_min).toBe(15);
  });
  it('rejects overlaps, unknown fields, misaligned intervals, and ambiguous generated targets', () => {
    expect(() => parseHeatingProfile({ ...earlier, weekday: [[4, 9], [8, 10]] })).toThrow('overlap');
    expect(() => parseHeatingProfile({ ...earlier, typo: 1 })).toThrow('Unknown');
    expect(() => applyHeatingProfile(JSON.stringify(model()), parseHeatingProfile({ ...earlier, weekday: [[4.1, 9]] }))).toThrow('align');
    const input = model(); input.Control.space.schedule.weekday[14] = 21;
    expect(() => applyHeatingProfile(JSON.stringify(input), parseHeatingProfile(earlier))).toThrow('single');
  });
  it('retains both inputs and the selected profile around actual adapter calls', async () => {
    const input = JSON.stringify(model());
    const stages: string[] = [];
    const files = await runHeatingProfile('{}', { type: 'reference' }, async (data, stage) => {
      stages.push(stage);
      if (stage === 'preprocess') return { 'actual__preproc.json': input };
      expect(data).toBe(input);
      return { 'core__results.csv': 'energy' };
    });
    expect(stages).toEqual(['preprocess', 'core']);
    expect(files['fhs-preprocessed.json']).toBe(input);
    expect(files['heating-input.json']).toBe(input);
    expect(JSON.parse(files['heating-profile.json'])).toEqual({ type: 'reference' });
  });
});

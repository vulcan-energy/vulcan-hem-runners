// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { webcrypto } from 'node:crypto';
beforeEach(() => vi.stubGlobal('crypto', webcrypto));
import { buildPythonFhsComplianceReport } from './pythonFhsCompliance';
import { executePreparedJobs } from './preparedRun';
import { HEM_TARGETS, type HemTargetManifest } from './targets';

const modes = ['actual', 'actual-fee', 'notional', 'notional-fee'];
// Tiny synthetic wrapper output records; no model, calculation or annual fixture.
const files = () => ({
  'model__FHS__postproc_summary.csv': ',,Total\r\nDER,kgCO2/m2,-2\r\nDPER,kWh/m2,0\r\n',
  'model__FHS_notional__postproc_summary.csv': ',,Total\nTER,kgCO2/m2,-1\nTPER,kWh/m2,0\n',
  'model__FHS_FEE__postproc.csv': 'Fabric Energy Efficiency,kWh / m2.yr,41.5\n',
  'model__FHS_FEE_notional__postproc.csv': 'Fabric Energy Efficiency,kWh / m2.yr,40\n',
});
const expected = {
  dwelling_emission_rate: -2, target_emission_rate: -1, emission_rate_compliant: true,
  dwelling_primary_energy_rate: 0, target_primary_energy_rate: 0, primary_energy_rate_compliant: true,
  dwelling_fabric_energy_efficiency: 41.5, target_fabric_energy_efficiency: 40, fabric_energy_efficiency_compliant: false,
};

describe('Python four-mode compliance summary', () => {
  it('maps exact wrapper rates independently of the input conversion profile', () => {
    expect(buildPythonFhsComplianceReport(files(), modes)).toEqual(expected);
  });
  it('preserves negative rates, zero, scientific notation and equality without sign correction or rounding', () => {
    const output = files();
    output['model__FHS__postproc_summary.csv'] = ',,Total\nDER,kgCO2/m2,-1e-3\nDPER,kWh/m2,0\n';
    output['model__FHS_notional__postproc_summary.csv'] = ',,Total\nTER,kgCO2/m2,-2e-3\nTPER,kWh/m2,0\n';
    const result = buildPythonFhsComplianceReport(output, [...modes].reverse())!;
    expect(result.dwelling_emission_rate).toBe(-0.001);
    expect(result.emission_rate_compliant).toBe(false);
    expect(result.primary_energy_rate_compliant).toBe(true);
  });
  it('does not fabricate a combined report for individual or partial modes', () => {
    for (const selected of [['actual'], ['actual-fee'], ['notional'], ['notional-fee'], ['actual', 'notional']]) {
      expect(buildPythonFhsComplianceReport({}, selected)).toBeUndefined();
    }
  });
  it.each(Object.keys(files()))('requires %s before producing a combined report', path => {
    const output: Record<string, string> = files(); delete output[path];
    expect(() => buildPythonFhsComplianceReport(output, modes)).toThrow(path);
  });
  it.each(['', 'NaN', 'Infinity', '1oops'])('rejects invalid emitted rate %j instead of making a zero/pass result', value => {
    const output = files(); output['model__FHS__postproc_summary.csv'] = `,,Total\nDER,kgCO2/m2,${value}\nDPER,kWh/m2,0\n`;
    expect(() => buildPythonFhsComplianceReport(output, modes)).toThrow('DER');
  });
  it.each([
    ',,Total\nDER,kgCO2/m2,2\nDER,kgCO2/m2,3\nDPER,kWh/m2,0\n',
    ',,Total\nDER,wrong units,2\nDPER,kWh/m2,0\n',
    ',,Total\nTER,kgCO2/m2,2\nDPER,kWh/m2,0\n',
    ',,Total\nDER,kgCO2/m2,2,extra\nDPER,kWh/m2,0\n',
  ])('rejects ambiguous or incompatible wrapper records', csv => {
    const output = files(); output['model__FHS__postproc_summary.csv'] = csv;
    expect(() => buildPythonFhsComplianceReport(output, modes)).toThrow('DER');
  });
  it('marks a missing four-mode rate as failed rather than completing with a fabricated summary', async () => {
    const target = HEM_TARGETS.find(target => target.conversionProfile === 'python_fhs_a9')!;
    const manifest = { bundleId: target.id, conversionProfile: target.conversionProfile, modes } as HemTargetManifest;
    const saved = new Map<string, string>();
    await expect(executePreparedJobs(manifest, target.id, 'synthetic', [{ name: 'tiny', input: '{}', modes, outputDirectory: 'output/tiny' }], async () => {
      buildPythonFhsComplianceReport({}, modes);
      return {};
    }, async (path, content) => { saved.set(path, content); }, () => {})).rejects.toThrow('missing');
    expect(JSON.parse(saved.get('output/tiny/target-run.json')!).status).toBe('failed');
    expect(saved.has('output/tiny/fhs_compliance_report.json')).toBe(false);
  });

  it('adds the summary to the completed output inventory and retains original mode records', async () => {
    const target = HEM_TARGETS.find(target => target.conversionProfile === 'python_fhs_a8')!;
    const manifest = { bundleId: target.id, conversionProfile: target.conversionProfile, modes } as HemTargetManifest;
    const saved = new Map<string, string>();
    await executePreparedJobs(manifest, target.id, 'synthetic', [{ name: 'tiny', input: '{}', modes, outputDirectory: 'output/tiny' }], async () => {
      const raw = files();
      return { ...raw, 'fhs_compliance_report.json': JSON.stringify(buildPythonFhsComplianceReport(raw, modes)) };
    }, async (path, content) => { saved.set(path, content); }, () => {});
    const record = JSON.parse(saved.get('output/tiny/target-run.json')!);
    expect(record.status).toBe('complete');
    expect(record.files).toEqual([...Object.keys(files()), 'fhs_compliance_report.json']);
    expect(JSON.parse(saved.get('output/tiny/fhs_compliance_report.json')!)).toEqual(expected);
    expect(saved.get('output/tiny/model__FHS__postproc_summary.csv')).toBe(files()['model__FHS__postproc_summary.csv']);
  });
});

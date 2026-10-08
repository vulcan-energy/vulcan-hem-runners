// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import Papa from 'papaparse';

export interface PythonFhsComplianceReport {
  dwelling_emission_rate: number;
  target_emission_rate: number;
  emission_rate_compliant: boolean;
  dwelling_primary_energy_rate: number;
  target_primary_energy_rate: number;
  primary_energy_rate_compliant: boolean;
  dwelling_fabric_energy_efficiency: number;
  target_fabric_energy_efficiency: number;
  fabric_energy_efficiency_compliant: boolean;
}

/**
 * Pinned Python a8/a9 write_postproc_summary_file and apply_fhs_FEE_postprocessing
 * emit these quantities directly (their writer ASTs are identical). Preserve the
 * wrapper's negative/zero values and precision; do not recalculate from core CSVs
 * or FHS_metrics.json, which contains EER rather than these compliance rates.
 * Comparisons match the existing Rust FhsComplianceResponse::build_from contract.
 */
export function buildPythonFhsComplianceReport(
  files: Record<string, string>,
  modes: string[],
): PythonFhsComplianceReport | undefined {
  if (!['actual', 'actual-fee', 'notional', 'notional-fee'].every(mode => modes.includes(mode))) return undefined;
  const rows = (path: string): string[][] => {
    if (!Object.prototype.hasOwnProperty.call(files, path)) throw new Error(`Python compliance summary is missing ${path}`);
    const parsed = Papa.parse<string[]>(files[path], { delimiter: ',', skipEmptyLines: 'greedy' });
    if (parsed.errors.length) throw new Error(`Invalid Python compliance CSV ${path}: ${parsed.errors[0].message}`);
    return parsed.data;
  };
  const rate = (records: string[][], name: string, unit: string, path: string): number => {
    const matching = records.filter(row => row[0] === name);
    if (matching.length !== 1 || matching[0].length !== 3 || matching[0][1] !== unit) throw new Error(`Python compliance summary requires one ${name} row in ${unit} from ${path}`);
    const text = matching[0][2].trim();
    const value = Number(text);
    if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(text) || !Number.isFinite(value)) throw new Error(`Invalid ${name} rate in Python compliance output ${path}`);
    return value;
  };
  // pythonFhsWorker writes model.json, so the upstream CLI uses the model prefix.
  const actualPath = 'model__FHS__postproc_summary.csv';
  const notionalPath = 'model__FHS_notional__postproc_summary.csv';
  const feePath = 'model__FHS_FEE__postproc.csv';
  const notionalFeePath = 'model__FHS_FEE_notional__postproc.csv';
  const actual = rows(actualPath);
  const notional = rows(notionalPath);
  const dwelling_emission_rate = rate(actual, 'DER', 'kgCO2/m2', actualPath);
  const target_emission_rate = rate(notional, 'TER', 'kgCO2/m2', notionalPath);
  const dwelling_primary_energy_rate = rate(actual, 'DPER', 'kWh/m2', actualPath);
  const target_primary_energy_rate = rate(notional, 'TPER', 'kWh/m2', notionalPath);
  const dwelling_fabric_energy_efficiency = rate(rows(feePath), 'Fabric Energy Efficiency', 'kWh / m2.yr', feePath);
  const target_fabric_energy_efficiency = rate(rows(notionalFeePath), 'Fabric Energy Efficiency', 'kWh / m2.yr', notionalFeePath);
  return {
    dwelling_emission_rate, target_emission_rate,
    emission_rate_compliant: dwelling_emission_rate <= target_emission_rate,
    dwelling_primary_energy_rate, target_primary_energy_rate,
    primary_energy_rate_compliant: dwelling_primary_energy_rate <= target_primary_energy_rate,
    dwelling_fabric_energy_efficiency, target_fabric_energy_efficiency,
    fabric_energy_efficiency_compliant: dwelling_fabric_energy_efficiency <= target_fabric_energy_efficiency,
  };
}

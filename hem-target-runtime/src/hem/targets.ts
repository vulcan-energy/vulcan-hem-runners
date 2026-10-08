// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

/** Input contracts describe semantics, independently of the runner implementation.
 * Legacy names remain protocol identifiers for already published immutable bundles.
 */
export type ConversionProfile =
  | 'divided_opening_half_partition_v1'
  | 'physical_opening_full_partition_v1'
  | 'physical_opening_full_partition_party_wall_u_v1'
  | 'current_rust_fhs' | 'python_fhs_a8' | 'python_fhs_a9';

// Only the implemented wrapper is registered. Adding a family requires its own
// qualified adapter; this is not a claim of EPC/Core execution support.
const WRAPPERS = {
  fhs: {
    profile: 'fhs',
    modes: ['actual', 'actual-fee', 'notional', 'notional-fee'],
    scenarioModes: {
      fhs_assumptions: ['actual'],
      fhs_compliance: ['actual', 'actual-fee', 'notional', 'notional-fee'],
    },
    pythonModule: 'bin.fhs',
    coreDistribution: 'hem-core',
    wrapperDistribution: 'hem-fhs-wrapper',
  },
} as const;

export interface HemTarget {
  id: string;
  label: string;
  runtime: 'rust' | 'python';
  wrapper: keyof typeof WRAPPERS;
  /** Display version retained for consumers of the existing catalogue. */
  version: string;
  engineVersion: string;
  wrapperVersion: string;
  engineCommit: string;
  wrapperCommit: string;
  conversionProfile: ConversionProfile;
}

export function targetWrapper(target: HemTarget) {
  const wrapper = WRAPPERS[target.wrapper];
  if (!wrapper) throw new Error(`Unsupported HEM wrapper: ${target.wrapper}`);
  return wrapper;
}

/** Keep the exact wire profile expected by this target's pinned preparation WASM. */
export function targetPreparationContract(target: HemTarget) {
  return {
    profile: targetWrapper(target).profile,
    conversion_profile: target.conversionProfile,
    version_metadata: { hem_core_version: target.engineVersion, fhs_wrapper_version: target.wrapperVersion },
  };
}

export function targetScenarioModes(target: HemTarget, choice: string): string[] {
  const modes = Object.entries(targetWrapper(target).scenarioModes).find(([name]) => name === choice)?.[1];
  if (!modes) throw new Error(`Wrapper ${choice} is incompatible with the selected ${target.wrapper.toUpperCase()} target`);
  return [...modes];
}

/** Version identity belongs to the selected target, not the Rust worker build. */
export function assertRustTargetVersions(target: HemTarget, engineVersion: unknown, wrapperVersion: unknown): void {
  if (engineVersion !== target.engineVersion || wrapperVersion !== target.wrapperVersion) throw new Error('Rust runtime version mismatch');
}

/** Immutable pins. A prepared release manifest binds these to executable artifacts. */
export const HEM_TARGETS = [
  { id: 'rust-fhs-a7-62d3df70-c5ba2673-v1', label: 'Rust HEM:FHS 1.0.0a7', runtime: 'rust', version: '1.0.0a7', engineVersion: '1.0.0a7', wrapperVersion: '1.0.0a7', wrapper: 'fhs',
    engineCommit: '62d3df705690f33b3fc3e905c9971d4f3743bf2e', wrapperCommit: 'c5ba2673fbd886cfe4fb528f61b376bdf406ebbd', conversionProfile: 'current_rust_fhs' },
  { id: 'python-fhs-a9-918addad-b9b90138-v1', label: 'Python HEM:FHS 1.0.0a9', runtime: 'python', version: '1.0.0a9', engineVersion: '1.0.0a9', wrapperVersion: '1.0.0a9', wrapper: 'fhs',
    engineCommit: '918addad2f9ffbcd9c7a4ed73497d047210be2d2', wrapperCommit: 'b9b901382feb0e5d27f969532842250147fb4068', conversionProfile: 'python_fhs_a9' },
  { id: 'python-fhs-a8-f2ab6cf7-8ca182b0-v1', label: 'Python HEM:FHS 1.0.0a8', runtime: 'python', version: '1.0.0a8', engineVersion: '1.0.0a8', wrapperVersion: '1.0.0a8', wrapper: 'fhs',
    engineCommit: 'f2ab6cf78860c36d260cb21396b747cc372f2c9d', wrapperCommit: '8ca182b0171a3a928059a9c0ec0145ecefe3f4e9', conversionProfile: 'python_fhs_a8' },
] as const satisfies readonly HemTarget[];
export const RECOMMENDED_HEM_TARGET = HEM_TARGETS[0].id;
export function resolveHemTarget(id: string): HemTarget {
  const target = HEM_TARGETS.find(target => target.id === id);
  if (!target) throw new Error(`HEM target '${id}' is unavailable. Select a supported target explicitly; saved runs are never upgraded automatically.`);
  return target;
}
export interface TargetArtifact { path: string; sha256: string; bytes: number; }
export interface HemTargetManifest {
  bundleId: string;
  protocol: 1;
  engineCommit: string;
  wrapperCommit: string;
  conversionProfile: HemTarget['conversionProfile'];
  preparation: { javascript: string; wasm: string; schema: string; defaults: string };
  runtime: { javascript: string; wasm?: string; index?: string; wheels?: string[]; packages?: string[]; requiredVersions?: Record<string, string> };
  modes: string[];
  artifacts: TargetArtifact[];
}
/** Source archives and notices stay in the published release, outside executable cache readiness. */
export function modelArtifacts(manifest: HemTargetManifest): TargetArtifact[] {
  return manifest.artifacts.filter(artifact => !artifact.path.startsWith('source/') && !artifact.path.startsWith('notices/'));
}
export function validateTargetManifest(manifest: HemTargetManifest, target: HemTarget): void {
  if (manifest.bundleId !== target.id || manifest.protocol !== 1 || manifest.engineCommit !== target.engineCommit ||
      manifest.wrapperCommit !== target.wrapperCommit || manifest.conversionProfile !== target.conversionProfile) {
    throw new Error(`Target manifest identity mismatch for ${target.id}`);
  }
  const paths = new Set<string>();
  const sizesByHash = new Map<string, number>();
  const isRelativePath = (path: string) => typeof path === 'string' && /^[a-zA-Z0-9_./+-]+$/.test(path)
    && path.split('/').every(segment => segment !== '' && segment !== '.' && segment !== '..');
  for (const artifact of manifest.artifacts) {
    if (!isRelativePath(artifact.path) ||
        !/^[a-f0-9]{64}$/.test(artifact.sha256) || !Number.isSafeInteger(artifact.bytes) || artifact.bytes <= 0 || paths.has(artifact.path)) {
      throw new Error(`Invalid artifact in ${target.id}: ${artifact.path}`);
    }
    const priorSize = sizesByHash.get(artifact.sha256);
    if (priorSize !== undefined && priorSize !== artifact.bytes) throw new Error(`Conflicting sizes for pinned artifact hash ${artifact.sha256}`);
    sizesByHash.set(artifact.sha256, artifact.bytes);
    paths.add(artifact.path);
  }
  for (const path of [...Object.values(manifest.preparation), manifest.runtime.javascript, ...(manifest.runtime.wasm ? [manifest.runtime.wasm] : []), ...(manifest.runtime.wheels ?? [])]) {
    if (path.startsWith('source/') || path.startsWith('notices/')) throw new Error(`Runtime/preparation cannot reference reserved source/notice material: ${path}`);
    if (!paths.has(path)) throw new Error(`Target manifest is missing required artifact ${path}`);
  }
  if (!manifest.modes.length || new Set(manifest.modes).size !== manifest.modes.length || manifest.modes.some(mode => !(targetWrapper(target).modes as readonly string[]).includes(mode))) throw new Error('Unsupported or duplicate FHS calculation modes');
  if (target.runtime === 'rust') {
    if (!manifest.runtime.wasm) throw new Error('Rust target manifest is missing its runtime WASM artifact');
    if (manifest.modes.some(mode => mode !== 'actual') && manifest.modes.length !== targetWrapper(target).modes.length) throw new Error('Rust target modes require actual alone or the complete compliance set');
  } else {
    const { index, wheels, requiredVersions } = manifest.runtime;
    if (index?.startsWith('source/') || index?.startsWith('notices/')) throw new Error('Python runtime cannot use the reserved source/notice directory');
    if (!index?.endsWith('/') || !isRelativePath(index.slice(0, -1))) throw new Error('Python runtime index must be a relative directory ending in /');
    if (!wheels?.length || new Set(wheels).size !== wheels.length) throw new Error('Python runtime requires its exact wheel artifacts');
    for (const file of ['pyodide.asm.js', 'pyodide.asm.wasm', 'pyodide-lock.json', 'python_stdlib.zip']) {
      if (!paths.has(`${index}${file}`)) throw new Error(`Python runtime is missing pinned artifact ${index}${file}`);
    }
    if (requiredVersions?.[targetWrapper(target).coreDistribution] !== target.engineVersion || requiredVersions?.[targetWrapper(target).wrapperDistribution] !== target.wrapperVersion) {
      throw new Error(`Python runtime must pin matching engine ${target.engineVersion} and wrapper ${target.wrapperVersion} distributions`);
    }
  }
}

/** One target for every model in a batch; absent provenance is never guessed. */
export function resolveBatchTarget(ids: Array<string | undefined>, savedTarget?: string): HemTarget {
  if (!ids.length || ids.some(id => !id)) throw new Error('A base model has no saved HEM model version. Open and save each base model in the Geometry editor, then create this batch.');
  const unique = new Set(ids);
  if (unique.size !== 1) throw new Error('A batch cannot mix HEM targets. Create a separate batch for each target.');
  const id = ids[0]!;
  if (savedTarget && savedTarget !== id) throw new Error('Saved batch target differs from its source. Explicitly refresh and resave the batch.');
  return resolveHemTarget(id);
}

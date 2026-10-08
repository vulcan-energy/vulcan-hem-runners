// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { pythonFhsArguments } from './pythonFhsArguments';
import { buildPythonFhsComplianceReport } from './pythonFhsCompliance';
import { TargetAssets } from './targetAssets';
import { modelArtifacts, resolveHemTarget, targetWrapper, validateTargetManifest, type HemTargetManifest } from './targets';
import { BatchWorkerFileSystemAcknowledgements } from '../workers/batchWorkerFileSystemBridge';
import { FS_REQUEST_TYPE, FS_RESPONSE_TYPE } from '../workers/workerProtocol';

import { executePreparedJobs, type PreparedHemJob } from './preparedRun';
type PythonRuntime = {
  loadPackage(names: string | string[]): Promise<void>;
  runPython(code: string): string;
  runPythonAsync(code: string): Promise<unknown>;
  FS: { mkdirTree(path: string): void; writeFile(path: string, data: string): void; readdir(path: string): string[]; readFile(path: string, options: {encoding: 'utf8'}): string; };
};
let runtime: PythonRuntime | undefined;
let manifest: HemTargetManifest | undefined;
let busy = false;
const acknowledgements = new BatchWorkerFileSystemAcknowledgements();
const send = (message: unknown) => self.postMessage(message);
async function write(path: string, content: string): Promise<void> {
  const request = { type: FS_REQUEST_TYPE, operation: 'write_file' as const, path, content, messageId: crypto.randomUUID() };
  acknowledgements.register(request);
  send(request);
  await acknowledgements.waitForPending();
}
async function initialize(bundle: HemTargetManifest): Promise<void> {
  const target = resolveHemTarget(bundle.bundleId);
  validateTargetManifest(bundle, target);
  if (target.runtime !== 'python' || target.wrapper !== 'fhs') throw new Error('Python worker requires a Python FHS target');
  const assets = await TargetAssets.open();
  const responses = new Map<string, Response>();
  await Promise.all(modelArtifacts(bundle).map(async artifact => {
    const bytes = await assets.bytes(artifact);
    const url = new URL(`/hem-targets/${bundle.bundleId}/${artifact.path}`, self.location.origin).href;
    responses.set(url, new Response(bytes as Uint8Array<ArrayBuffer>, { headers: { 'Content-Type': artifact.path.endsWith('.wasm') ? 'application/wasm' : 'application/octet-stream' } }));
  }));
  // Every Pyodide dependency request uses verified bytes. Unknown transitive downloads
  // fail visibly, including accidental PyPI resolution and offline cache eviction.
  self.fetch = async (input) => {
    const url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url, self.location.origin).href;
    const response = responses.get(url);
    if (!response) throw new Error(`Python attempted to load an artifact outside its pinned bundle: ${url}`);
    return response.clone();
  };
  const importAsset = async (path: string) => {
    const response = responses.get(new URL(`/hem-targets/${bundle.bundleId}/${path}`, self.location.origin).href);
    if (!response) throw new Error(`Missing executable artifact ${path}`);
    const url = URL.createObjectURL(new Blob([await response.arrayBuffer()], { type: 'text/javascript' }));
    try {
      // This module is executed from the verified target artifact bytes, not a bundled app path.
      // react-doctor-disable-next-line react-doctor/no-dynamic-import-path -- Pyodide JS is imported from a verified blob URL.
      return await import(/* @vite-ignore */ url);
    } finally { URL.revokeObjectURL(url); }
  };
  const index = bundle.runtime.index;
  if (!index || !bundle.runtime.wheels?.length || !bundle.runtime.requiredVersions) throw new Error('Incomplete Python runtime manifest');
  await importAsset(`${index}pyodide.asm.js`);
  const { loadPyodide } = await importAsset(bundle.runtime.javascript);
  runtime = await loadPyodide({ indexURL: new URL(`/hem-targets/${bundle.bundleId}/${index}`, self.location.origin).href,
    stdout: (message: string) => send({ type: 'worker_log', message }), stderr: (message: string) => send({ type: 'worker_log', message }) });
  const py = runtime!;
  // Pyodide install order is a runtime dependency: bootstrap micropip, install numpy,
  // load its declared packages, then install the remaining pinned wheels.
  await py.loadPackage('micropip');
  const install = (paths: string[]) => py.runPythonAsync(`import micropip\nawait micropip.install(${JSON.stringify(paths.map(path => new URL(`/hem-targets/${bundle.bundleId}/${path}`, self.location.origin).href))}, index_urls=[])`);
  // react-doctor-disable-next-line react-doctor/async-parallel -- the numpy wheel must be installed before Pyodide packages and remaining wheels.
  await install(bundle.runtime.wheels.filter(path => path.split('/').pop()!.startsWith('numpy-')));
  await py.loadPackage(bundle.runtime.packages ?? []);
  await install(bundle.runtime.wheels.filter(path => !path.split('/').pop()!.startsWith('numpy-')));
  py.runPython(`import os\nos.environ['MPLBACKEND']='Agg'\nfrom ${targetWrapper(target).pythonModule} import main`);
  const versions = JSON.parse(py.runPython("import importlib.metadata as m,json\njson.dumps({d.metadata['Name'].lower().replace('_','-'):d.version for d in m.distributions()})")) as Record<string,string>;
  for (const [name, version] of Object.entries(bundle.runtime.requiredVersions)) {
    if (versions[name.toLowerCase().replace(/_/g,'-')] !== version) throw new Error(`Python runtime identity mismatch for ${name}`);
  }
  manifest = bundle;
  send({ type: 'worker_ready', targetBundleId: bundle.bundleId });
}
async function run(jobId: string, bundleId: string, jobs: PreparedHemJob[]): Promise<void> {
  if (!runtime || !manifest) throw new Error('Python worker is not initialized');
  await executePreparedJobs(manifest, bundleId, jobId, jobs, async (input, modes, weather) => {
    const py = runtime!;
    const folder = `/runs/${crypto.randomUUID()}`;
    py.FS.mkdirTree(folder); py.FS.writeFile(`${folder}/model.json`, input);
    try {
      if (weather) py.FS.writeFile(`${folder}/weather.epw`, weather.epw);
      const args = pythonFhsArguments(`${folder}/model.json`, modes, weather ? `${folder}/weather.epw` : undefined);
      await py.runPythonAsync(`main(${JSON.stringify(args)}, standalone_mode=False)`);
      const output = `${folder}/model__results`;
      const files = Object.fromEntries(py.FS.readdir(output).flatMap(name =>
        name !== '.' && name !== '..' ? [[name, py.FS.readFile(`${output}/${name}`, { encoding: 'utf8' })]] : []));
      const compliance = buildPythonFhsComplianceReport(files, modes);
      if (compliance) files['fhs_compliance_report.json'] = JSON.stringify(compliance);
      return files;
    } finally { await py.runPythonAsync(`import shutil\nshutil.rmtree(${JSON.stringify(folder)})`); }
  }, write, (completed, total) => send({ type: 'progress', jobId, progress: { completed, total, status: 'Calculating' } }));
  send({ type: 'complete', jobId, targetBundleId: bundleId, result: { status: 'success', targetBundleId: bundleId } });
}
self.onmessage = async ({ data }) => {
  if (data.type === FS_RESPONSE_TYPE) { acknowledgements.acknowledge(data.messageId, data.error ?? null); return; }
  if (busy) { send({ type: 'error', jobId: data.jobId, error: 'A calculation worker is already active' }); return; }
  busy = true;
  try {
    if (data.type === 'initialize') await initialize(data.manifest);
    else if (data.type === 'run_prepared_batch') await run(data.jobId, data.targetBundleId, data.jobs);
    else throw new Error(`Unsupported Python worker message ${data.type}`);
  } catch (error) { send({ type: data.type === 'initialize' ? 'init_error' : 'error', jobId: data.jobId, error: String(error) }); }
  finally { busy = false; }
};

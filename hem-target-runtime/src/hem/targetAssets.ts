// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { modelArtifacts, resolveHemTarget, validateTargetManifest, type HemTargetManifest, type TargetArtifact } from './targets';

const CACHE = 'vulcan-hem-artifacts-v1';
export async function sha256(bytes: Uint8Array): Promise<string> {
  return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes as Uint8Array<ArrayBuffer>)), b => b.toString(16).padStart(2, '0')).join('');
}
const keyFor = (artifact: TargetArtifact) => new URL(`/__hem_content/${artifact.sha256}`, globalThis.location.origin).href;
async function verified(response: Response, artifact: TargetArtifact, preserveBody = true): Promise<boolean> {
  const bytes = new Uint8Array(await (preserveBody ? response.clone() : response).arrayBuffer());
  return bytes.byteLength === artifact.bytes && await sha256(bytes) === artifact.sha256;
}
export class TargetAssets {
  constructor(private readonly cache: Cache, private readonly fetcher: typeof fetch = (...args) => globalThis.fetch(...args)) {}
  static async open(): Promise<TargetAssets> { return new TargetAssets(await caches.open(CACHE)); }
  async inspect(manifest: HemTargetManifest, signal?: AbortSignal): Promise<{ missingBytes: number; missing: TargetArtifact[] }> {
    signal?.throwIfAborted();
    const unique = [...new Map(modelArtifacts(manifest).map(artifact => [artifact.sha256, artifact])).values()];
    const missing: TargetArtifact[] = [];
    // Limit simultaneous cache bodies/hashes; Python bundles contain hundreds of files.
    for (let offset = 0; offset < unique.length; offset += 4) {
      signal?.throwIfAborted();
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- bounded batches avoid buffering every runtime artifact simultaneously.
      const results = await Promise.all(unique.slice(offset, offset + 4).map(async artifact => {
        signal?.throwIfAborted();
        const key = keyFor(artifact);
        const response = await this.cache.match(key);
        signal?.throwIfAborted();
        const valid = response && await verified(response, artifact, false);
        signal?.throwIfAborted();
        if (valid) return undefined;
        if (response) await this.cache.delete(key);
        return artifact;
      }));
      missing.push(...results.filter((artifact): artifact is TargetArtifact => artifact !== undefined));
    }
    signal?.throwIfAborted();
    return { missing, missingBytes: missing.reduce((bytes, artifact) => bytes + artifact.bytes, 0) };
  }
  async download(manifest: HemTargetManifest, progress: (downloaded: number, total: number) => void, signal?: AbortSignal): Promise<void> {
    const { missing, missingBytes } = await this.inspect(manifest, signal);
    signal?.throwIfAborted();
    let downloaded = 0;
    let next = 0;
    let failure: unknown;
    const controller = new AbortController();
    const abort = () => controller.abort(signal?.reason);
    signal?.addEventListener('abort', abort, { once: true });
    const activeSignal = controller.signal;
    const acquire = async (artifact: TargetArtifact) => {
      activeSignal.throwIfAborted();
      const response = await this.fetcher(`/hem-targets/${manifest.bundleId}/${artifact.path}`, { signal: activeSignal, cache: 'no-store' });
      activeSignal.throwIfAborted();
      if (!response.ok) throw new Error(`Missing or corrupt HEM artifact: ${artifact.path}. Retry the download.`);
      const bytes = new Uint8Array(artifact.bytes);
      let received = 0;
      const append = (chunk: Uint8Array) => {
        activeSignal.throwIfAborted();
        if (received + chunk.byteLength > artifact.bytes) throw new Error(`Missing or corrupt HEM artifact size: ${artifact.path}. Retry the download.`);
        bytes.set(chunk, received);
        received += chunk.byteLength;
        downloaded += chunk.byteLength;
        progress(downloaded, missingBytes);
      };
      if (response.body) {
        const reader = response.body.getReader();
        const cancelReader = () => { void reader.cancel(activeSignal.reason).catch(() => {}); };
        activeSignal.addEventListener('abort', cancelReader, { once: true });
        try {
          while (true) {
            activeSignal.throwIfAborted();
            // react-doctor-disable-next-line react-doctor/async-await-in-loop -- stream chunks must be consumed in order for accurate byte progress.
            const { done, value } = await reader.read();
            activeSignal.throwIfAborted();
            if (done) break;
            append(value);
          }
        } catch (error) {
          await reader.cancel(error).catch(() => {});
          throw error;
        } finally {
          activeSignal.removeEventListener('abort', cancelReader);
          reader.releaseLock();
        }
      } else {
        append(new Uint8Array(await response.arrayBuffer()));
      }
      if (received !== artifact.bytes || await sha256(bytes) !== artifact.sha256) throw new Error(`Missing or corrupt HEM artifact: ${artifact.path}. Retry the download.`);
      activeSignal.throwIfAborted();
      await this.cache.put(keyFor(artifact), new Response(bytes, { headers: response.headers }));
      activeSignal.throwIfAborted();
    };
    const worker = async () => {
      try {
        while (next < missing.length) {
          activeSignal.throwIfAborted();
          const artifact = missing[next++];
          // react-doctor-disable-next-line react-doctor/async-await-in-loop -- each of four workers keeps one request active and acknowledges its verified cache write.
          await acquire(artifact);
        }
      } catch (error) {
        failure ??= error;
        controller.abort(error);
      }
    };
    try {
      signal?.throwIfAborted();
      progress(0, missingBytes);
      // react-doctor-disable-next-line react-doctor/async-defer-await -- workers record failure asynchronously; all must settle before checking it or returning.
      await Promise.all(Array.from({ length: Math.min(4, missing.length) }, worker));
      if (failure !== undefined) throw failure;
      activeSignal.throwIfAborted();
    } finally {
      signal?.removeEventListener('abort', abort);
    }
  }
  /** Load only the editor schema; selecting a version must not fetch its runtime. */
  async ensureBytes(bundleId: string, artifact: TargetArtifact): Promise<Uint8Array> {
    let response = await this.cache.match(keyFor(artifact));
    if (!response || !await verified(response, artifact)) {
      response = await this.fetcher(`/hem-targets/${bundleId}/${artifact.path}`, { cache: 'no-store' });
      if (!response.ok || !await verified(response, artifact)) {
        throw new Error(`Missing or corrupt HEM artifact: ${artifact.path}. Retry the download.`);
      }
      await this.cache.put(keyFor(artifact), response.clone());
    }
    return new Uint8Array(await response.arrayBuffer());
  }
  async bytes(artifact: TargetArtifact): Promise<Uint8Array> {
    const response = await this.cache.match(keyFor(artifact));
    if (!response || !await verified(response, artifact)) throw new Error(`Exact HEM artifact unavailable: ${artifact.path}. Download this target again.`);
    return new Uint8Array(await response.arrayBuffer());
  }
}

// Published release manifests must be pinned here by hash. No mutable remote catalogue.
// Entries are generated after the preparation/runtime build is qualified; missing pins fail closed.
export const RELEASE_MANIFEST_HASHES: Readonly<Record<string, string>> = {
  "rust-fhs-a7-62d3df70-c5ba2673-v1": "70a65af0e6be8ecd478d595731a659350a1acb49b6b50281c673d4a22c5e61f4",
  "python-fhs-a8-f2ab6cf7-8ca182b0-v1": "733928eb024be97ac6a9727fbaa256da1bc51d969c427d492007fb0835260e45",
  "python-fhs-a9-918addad-b9b90138-v1": "9a36fb5070dab79ef8f7cc9e64946f85c7cdaaa83bb61d2689e6492c805f1b98"
};
export async function loadTargetManifest(id: string): Promise<HemTargetManifest> {
  const target = resolveHemTarget(id);
  const hash = RELEASE_MANIFEST_HASHES[id];
  if (!hash) throw new Error(`${target.label} is not available in this app build.`);
  const response = await fetch(`/hem-targets/${id}/manifest.json`, { cache: 'no-store' });
  if (!response.ok) throw new Error(`Target manifest unavailable: ${id}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (await sha256(bytes) !== hash) throw new Error(`Target manifest hash mismatch: ${id}`);
  const manifest = JSON.parse(new TextDecoder().decode(bytes)) as HemTargetManifest;
  validateTargetManifest(manifest, target);
  return manifest;
}

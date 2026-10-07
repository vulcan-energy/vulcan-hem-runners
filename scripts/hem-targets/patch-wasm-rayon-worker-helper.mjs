#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// Approved runner build helper copy; subject to Vulcan Origin Terms v1.0. See scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
import { readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

const MARKER = "VULCAN_RAYON_FS_BRIDGE_VERSION";

const BRIDGE_BLOCK = `// Vulcan patch: Rayon child workers can emit FHS postprocessing files.
// wasm-bindgen-rayon regenerates this helper, so scripts/deploy/build-wasm.sh
// reapplies this bridge after every wasm-pack build.
const VULCAN_RAYON_FS_BRIDGE_VERSION = 1;
const VULCAN_FS_REQUEST_TYPE = 'file_system';

function vulcanRayonFsMessageId(operation) {
  return \`rayon_\${operation}_\${Date.now()}_\${Math.random()}\`;
}

function postVulcanRayonFileRequest(operation, path, content) {
  const message = {
    type: VULCAN_FS_REQUEST_TYPE,
    operation,
    path,
    messageId: vulcanRayonFsMessageId(operation)
  };
  if (typeof content === 'string') {
    message.content = content;
  }
  self.postMessage(message);
  return { ok: true };
}

function installVulcanRayonFileBridge(scope) {
  if (typeof scope.write_file !== 'function') {
    scope.write_file = (path, content) => {
      return postVulcanRayonFileRequest('write_file', path, content);
    };
  }
  if (typeof scope.create_dir_all !== 'function') {
    scope.create_dir_all = (path) => {
      return postVulcanRayonFileRequest('create_dir_all', path);
    };
  }
  if (typeof scope.delete_file !== 'function') {
    scope.delete_file = (path) => {
      return postVulcanRayonFileRequest('delete_file', path);
    };
  }
}

installVulcanRayonFileBridge(self);

`;

const WORKER_CREATE_PATTERN =
  /(      const worker = new Worker\(url, \{\r?\n        type: 'module'\r?\n      \}\);\r?\n)(      worker\.postMessage\(workerInit\);)/;

const WORKER_FORWARDING_BLOCK = `      worker.addEventListener('message', ({ data }) => {
        if (data && data.type === VULCAN_FS_REQUEST_TYPE) {
          self.postMessage(data);
        }
      });
`;

function usage() {
  return [
    "Usage: node scripts/patch-wasm-rayon-worker-helper.mjs [--check] --root <dir>",
    "",
    "Finds wasm-bindgen-rayon workerHelpers.no-bundler.js files under <dir>",
    "and installs Vulcan's file-write bridge for Rayon child workers.",
  ].join("\n");
}

function parseArgs(argv) {
  const options = {
    check: false,
    roots: [],
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--check") {
      options.check = true;
      continue;
    }
    if (arg === "--root") {
      const root = argv[i + 1];
      if (!root) {
        throw new Error("--root requires a directory");
      }
      options.roots.push(root);
      i += 1;
      continue;
    }
    if (arg === "--help" || arg === "-h") {
      console.log(usage());
      process.exit(0);
    }
    throw new Error(`Unknown argument: ${arg}`);
  }

  if (options.roots.length === 0) {
    options.roots.push("web/pkg");
  }

  return options;
}

async function findHelpers(root) {
  const helpers = [];

  async function walk(dir) {
    let entries;
    try {
      entries = await readdir(dir, { withFileTypes: true });
    } catch (error) {
      if (error?.code === "ENOENT") {
        return;
      }
      throw error;
    }

    for (const entry of entries) {
      const entryPath = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        await walk(entryPath);
      } else if (
        entry.name === "workerHelpers.no-bundler.js" &&
        entryPath.includes(`${path.sep}wasm-bindgen-rayon-`)
      ) {
        helpers.push(entryPath);
      }
    }
  }

  await walk(root);
  return helpers;
}

function hasBridge(content) {
  return (
    content.includes(MARKER) &&
    content.includes("scope.write_file = (path, content) =>") &&
    content.includes("postVulcanRayonFileRequest('write_file', path, content)") &&
    content.includes("worker.addEventListener('message', ({ data }) =>") &&
    content.includes("data.type === VULCAN_FS_REQUEST_TYPE")
  );
}

function patchContent(content, helperPath) {
  let next = content;

  if (!next.includes(MARKER)) {
    if (!next.includes("function waitForMsgType")) {
      throw new Error(`${helperPath}: cannot find waitForMsgType insertion point`);
    }
    next = next.replace("function waitForMsgType", `${BRIDGE_BLOCK}function waitForMsgType`);
  }

  if (!next.includes("worker.addEventListener('message', ({ data }) =>")) {
    if (!WORKER_CREATE_PATTERN.test(next)) {
      throw new Error(`${helperPath}: cannot find Rayon worker creation block`);
    }
    next = next.replace(WORKER_CREATE_PATTERN, `$1${WORKER_FORWARDING_BLOCK}$2`);
  }

  if (!hasBridge(next)) {
    throw new Error(`${helperPath}: helper patch did not install the expected bridge`);
  }

  return next;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const helperPaths = [];

  for (const root of options.roots) {
    const resolvedRoot = path.resolve(root);
    helperPaths.push(...await findHelpers(resolvedRoot));
  }

  if (helperPaths.length === 0) {
    throw new Error(`No wasm-bindgen-rayon worker helper found under: ${options.roots.join(", ")}`);
  }

  const missing = [];
  const patched = [];

  for (const helperPath of helperPaths) {
    const content = await readFile(helperPath, "utf8");
    if (hasBridge(content)) {
      continue;
    }

    if (options.check) {
      missing.push(helperPath);
      continue;
    }

    await writeFile(helperPath, patchContent(content, helperPath));
    patched.push(helperPath);
  }

  if (missing.length > 0) {
    throw new Error(`Unpatched wasm-bindgen-rayon helper(s):\n${missing.join("\n")}`);
  }

  if (patched.length > 0) {
    console.log(`Patched ${patched.length} wasm-bindgen-rayon helper(s).`);
  } else {
    console.log(`All ${helperPaths.length} wasm-bindgen-rayon helper(s) already patched.`);
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});

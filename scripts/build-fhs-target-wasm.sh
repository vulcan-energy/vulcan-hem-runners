#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# This runner build helper is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
MANIFEST="$ROOT/runner-source.json"
OUT_FINAL="$(node -e 'const p=require("node:path"),f=require("node:fs"),x=p.resolve(process.argv[1]),parent=f.realpathSync(p.dirname(x));process.stdout.write(p.join(parent,p.basename(x)))' "${1:?Give an output directory outside the source tree}")"
case "$OUT_FINAL/" in "$ROOT/"*) echo 'Output must be outside the source repository' >&2; exit 2;; esac
[ ! -e "$OUT_FINAL" ] || { echo "Output already exists: $OUT_FINAL" >&2; exit 2; }
node "$ROOT/scripts/hem-targets/verify-runner-source-manifest.mjs" --manifest "$MANIFEST" --root "$ROOT" --phase before-build
OUT="$(mktemp -d "${TMPDIR:-/tmp}/hem-target-wasm.XXXXXX")"
trap 'rm -rf "$OUT"' EXIT
export RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+simd128,+nontrapping-fptoint,+sign-ext,+extended-const --cfg getrandom_backend="wasm_js"'
export VULCAN_WASM_BUILD_ID="runner-$(node -e 'const m=require(process.argv[1]);process.stdout.write(m.exportFilesSha256.slice(0,16))' "$MANIFEST")"
export VULCAN_SOURCE_GIT_COMMIT="$(node -e 'const m=require(process.argv[1]);process.stdout.write(m.sourceCommit)' "$MANIFEST")"
export VULCAN_SOURCE_TREE_SHA256="$(node -e 'const m=require(process.argv[1]);process.stdout.write(m.sourceTreeSha256)' "$MANIFEST")"
export VULCAN_SOURCE_TREE_DIRTY="$(node -e 'const m=require(process.argv[1]);process.stdout.write(String(m.sourceTreeDirty))' "$MANIFEST")"
mkdir -p "$OUT/pkg"
cd "$ROOT/hem_target_wrapper"
wasm-pack build --target web --release --out-name wasm_wrapper --out-dir "$OUT/pkg" . -- --no-default-features -Z build-std=std,panic_abort
cp "$ROOT/hem_target_wrapper/browser_env_shim.js" "$OUT/pkg/browser_env_shim.js"
node --input-type=module - "$OUT/pkg/wasm_wrapper.js" <<'NODE'
import fs from 'node:fs';
const file = process.argv[2];
const js = fs.readFileSync(file, 'utf8')
  .replaceAll("from 'env'", "from './browser_env_shim.js'")
  .replaceAll('from "env"', 'from "./browser_env_shim.js"');
fs.writeFileSync(file, js);
NODE
node "$ROOT/scripts/hem-targets/patch-wasm-rayon-worker-helper.mjs" --root "$OUT/pkg"
node "$ROOT/scripts/hem-targets/patch-wasm-rayon-worker-helper.mjs" --check --root "$OUT/pkg"
cat "$ROOT/scripts/hem-targets/licensing/runtime-header.js" "$OUT/pkg/wasm_wrapper.js" > "$OUT/pkg/wasm_wrapper.js.with-header"
mv "$OUT/pkg/wasm_wrapper.js.with-header" "$OUT/pkg/wasm_wrapper.js"
for NOTICE_FILE in LICENSE ADDITIONAL_TERMS.md NOTICE.md ATTRIBUTION.md TRADEMARKS.md FHS-MIT-LICENSE.md; do
  cp "$ROOT/$NOTICE_FILE" "$OUT/pkg/$NOTICE_FILE"
done
node "$ROOT/scripts/hem-targets/verify-runner-source-manifest.mjs" --manifest "$MANIFEST" --root "$ROOT" --phase after-build
[ ! -e "$OUT_FINAL" ] || { echo "Output appeared during build: $OUT_FINAL" >&2; exit 2; }
cp -R "$OUT/pkg" "$OUT_FINAL"
echo "Raw FHS target WASM candidate: $OUT_FINAL"
echo 'This raw build omits source-checkout post-build hashing/manifest decoration and is not qualified or released.'

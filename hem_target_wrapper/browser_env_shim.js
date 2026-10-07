/*
 * SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
 * Derived from the Vulcan browser WASM environment shim.
 */

/**
 * LLVM emits `import "env" "now"` as `() -> f64` for wasm32 timing helpers.
 * Bare `'env'` is invalid in native browser ES modules — we load this file instead.
 * Use monotonic clock (matches typical Rust `performance`-based expectations in workers).
 */
export function now() {
  return performance.now();
}

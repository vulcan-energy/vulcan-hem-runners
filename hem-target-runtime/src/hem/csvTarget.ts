// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { csvMetadataValue } from '@vulcan-community/geometry-editor/geometry/io/csvSectionRows';
import { defaultsReadPathAttempts } from '@vulcan-community/geometry-editor/lib/workspacePaths';
import { UNPINNED_CSV_TARGET } from './targets';

/**
 * The CSV's TargetBundleId. A CSV without one, or with an empty one, is an a7 model; the CSV is not
 * rewritten. Two different pins throw. The web app and the Node tools (bundled by
 * scripts/hem-targets/build-cli-runtime.mjs) both read the pin here.
 */
export function csvTarget(csv: string): string { return csvMetadataValue(csv, 'TargetBundleId') || UNPINNED_CSV_TARGET; }

/** The workspace paths target preparation tries for the CSV's DefaultsPath, in its order (captureDeclaredDefaults). */
export function csvDefaultsReadPaths(csv: string): string[] { return defaultsReadPathAttempts(csvMetadataValue(csv, 'DefaultsPath') ?? ''); }

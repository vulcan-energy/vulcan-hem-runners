// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

type JsonObject = Record<string, unknown>;

function isJsonObject(value: unknown): value is JsonObject {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/**
 * Build the projection accepted by the unchanged HEM/FHS contracts.
 *
 * The product model keeps `General.built_form` for SAP XML conversion. This function
 * parses into a separate object and removes exactly that named converter-owned field;
 * every other property is preserved for strict upstream validation.
 */
export function cloneForHemValidationJson(productModelJson: string): string {
  const validationModel = JSON.parse(productModelJson) as unknown;
  if (!isJsonObject(validationModel)) {
    throw new Error('Converted product model must be a JSON object');
  }

  if (isJsonObject(validationModel.General)) {
    delete validationModel.General.built_form;
  }

  return JSON.stringify(validationModel);
}

// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

/**
 * Format parameter snippet names by removing underscores and capitalizing words
 * @param name - The parameter name (e.g., "good_insulation")
 * @returns Formatted name (e.g., "Good Insulation")
 */
export const formatParameterName = (name: string): string => {
  return name
    .split('_')
    .map(word => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
    .join(' ');
};

/**
 * Format parameter category names by capitalizing words
 * @param name - The category name (e.g., "simplified_fabric")
 * @returns Formatted name (e.g., "Simplified Fabric")
 */
export const formatCategoryName = (name: string): string => {
  const overrides: Record<string, string> = {
    base_json: 'Base input file',
    sap_xml: 'SAP XML',
    compliance_settings: 'Compliance & Global settings',
    model_wrappers: 'Model Wrappers',
    additional_outputs: 'Additional Outputs',
  };
  if (overrides[name]) return overrides[name];
  return name
    .split('_')
    .map(word => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
    .join(' ');
};

/**
 * Generate markdown file path for a parameter snippet
 * @param category - The category name
 * @param snippetId - The snippet ID
 * @returns Path to the markdown file
 */
export const getParameterMarkdownPath = (category: string, snippetId: string): string => {
  return `input/batch_parameters/${category}/${snippetId}.md`;
};

/**
 * Generate markdown file path for a category README
 * @param category - The category name
 * @returns Path to the category README file
 */
export const getCategoryMarkdownPath = (category: string): string => {
  return `input/batch_parameters/${category}/README.md`;
}; 
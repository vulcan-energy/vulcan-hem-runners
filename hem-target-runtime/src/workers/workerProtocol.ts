// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

/**
 * Shared constants for the worker ↔ main-thread FS RPC protocol.
 *
 * Workers post messages with FS_REQUEST_TYPE to request file operations;
 * the main thread replies with FS_RESPONSE_TYPE.
 *
 * Centralised here so a typo in either side causes a compile-time
 * reference rather than a silent 30-second timeout.
 */

export const FS_REQUEST_TYPE = 'file_system' as const;
export const FS_RESPONSE_TYPE = 'file_system_response' as const;

export type FileSystemOperation =
  | 'write_file'
  | 'read_file'
  | 'file_exists'
  | 'delete_file'
  | 'create_dir_all'
  | 'list_dir';

type BaseFileSystemRequestMessage = {
  type: typeof FS_REQUEST_TYPE;
  operation: FileSystemOperation;
  path: string;
  messageId: string;
};

export type FileSystemRequestMessage =
  | (BaseFileSystemRequestMessage & { operation: 'write_file'; content: string })
  | (BaseFileSystemRequestMessage & {
      operation: Exclude<FileSystemOperation, 'write_file'>;
      content?: undefined;
    });

export type FileSystemResponseMessage = {
  type: typeof FS_RESPONSE_TYPE;
  messageId?: string;
  result?: unknown;
  error?: string | null;
};

export type GeometryWorkerValidationError = {
  code?: string;
  keyword?: string;
  path?: string;
  instancePath?: string;
  message?: string;
  category?: string;
  user_message?: string;
  technical_message?: string;
};

export type GeometryWorkerValidationResult = {
  is_valid?: boolean;
  errors?: GeometryWorkerValidationError[];
} & Record<string, unknown>;

export type GeometryWorkerPartFPreflightResult = {
  is_valid?: boolean;
  errors?: Array<GeometryWorkerValidationError | null>;
  raw_engine_error?: unknown;
} & Record<string, unknown>;

export type GeometryWorkerTemplateCompatibility = {
  warnings: string[];
  foundTypes: string[];
  hasRequiredRootSections: boolean;
};

export type GeometryWorkerResultMessage = {
  type: 'result';
  ok: boolean;
  name: string;
  csvSaved?: boolean;
  jsonSaved?: boolean;
  finalCsv?: string;
  error?: string;
  validation?: GeometryWorkerValidationResult;
  partFPreflight?: GeometryWorkerPartFPreflightResult;
  strictValidationPassed?: boolean;
  schemaOmissions?: Array<{ code: string; path: string; message: string }>;
  scenariosBaseModelEnabled?: boolean;
  compatibility?: GeometryWorkerTemplateCompatibility;
  debug?: {
    firstChars: string;
    firstLine: string;
    firstCharCode: number | null;
    length: number;
  };
};

export type GeometryWorkerMessage =
  | FileSystemRequestMessage
  | FileSystemResponseMessage
  | GeometryWorkerResultMessage;

export function isFileSystemRequestMessage(value: unknown): value is FileSystemRequestMessage {
  if (!value || typeof value !== 'object') return false;
  const record = value as Record<string, unknown>;
  if (record.type !== FS_REQUEST_TYPE) return false;
  if (typeof record.operation !== 'string') return false;
  if (typeof record.path !== 'string') return false;
  if (typeof record.messageId !== 'string') return false;
  if (record.operation === 'write_file') return typeof record.content === 'string';
  return (
    record.operation === 'read_file' ||
    record.operation === 'file_exists' ||
    record.operation === 'delete_file' ||
    record.operation === 'create_dir_all' ||
    record.operation === 'list_dir'
  );
}

export function isGeometryWorkerResultMessage(value: unknown): value is GeometryWorkerResultMessage {
  if (!value || typeof value !== 'object') return false;
  const record = value as Record<string, unknown>;
  return record.type === 'result' && typeof record.ok === 'boolean' && typeof record.name === 'string';
}

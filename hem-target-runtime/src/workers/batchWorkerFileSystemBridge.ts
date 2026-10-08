// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { FS_REQUEST_TYPE, type FileSystemRequestMessage } from './workerProtocol';

type WorkerFileSystemResult = { ok: true } | { error: string };

type DeleteBatchWorkerFileOptions = {
  fileCache: Map<string, string>;
  fileExistsCache: Map<string, boolean>;
  postMessage: (message: FileSystemRequestMessage) => void;
  createMessageId?: () => string;
};

type WorkspaceDeleteOperations = {
  deleteFile: (path: string) => Promise<void>;
  fileExists: (path: string) => Promise<boolean>;
};

type PendingAcknowledgement = {
  operation: 'write_file' | 'delete_file';
  path: string;
  promise: Promise<void>;
  resolve: () => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
  settled: boolean;
};

const DEFAULT_ACKNOWLEDGEMENT_TIMEOUT_MS = 30_000;

const defaultDeleteMessageId = (): string => `delete_${Date.now()}_${Math.random()}`;

export function deleteBatchWorkerFile(
  path: string,
  {
    fileCache,
    fileExistsCache,
    postMessage,
    createMessageId = defaultDeleteMessageId,
  }: DeleteBatchWorkerFileOptions,
): WorkerFileSystemResult {
  try {
    postMessage({
      type: FS_REQUEST_TYPE,
      operation: 'delete_file',
      path,
      messageId: createMessageId(),
    });
    fileCache.delete(path);
    fileExistsCache.set(path, false);
    return { ok: true };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

export async function deleteBatchWorkerWorkspaceFile(
  path: string,
  fileOperations: WorkspaceDeleteOperations,
): Promise<{ success: true }> {
  if (await fileOperations.fileExists(path)) {
    await fileOperations.deleteFile(path);
  }
  return { success: true };
}

export class BatchWorkerFileSystemAcknowledgements {
  private readonly pending = new Map<string, PendingAcknowledgement>();

  constructor(private readonly timeoutMs = DEFAULT_ACKNOWLEDGEMENT_TIMEOUT_MS) {
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
      throw new Error('Batch worker filesystem acknowledgement timeout must be positive');
    }
  }

  get pendingCount(): number {
    return this.pending.size;
  }

  register(request: FileSystemRequestMessage): void {
    if (request.operation !== 'write_file' && request.operation !== 'delete_file') {
      throw new Error(`Cannot acknowledge unsupported filesystem operation '${request.operation}'`);
    }
    if (this.pending.has(request.messageId)) {
      throw new Error(`Duplicate filesystem acknowledgement id '${request.messageId}'`);
    }

    let resolve!: () => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<void>((promiseResolve, promiseReject) => {
      resolve = promiseResolve;
      reject = promiseReject;
    });
    // A response may arrive before the batch reaches waitForPending(). Attach a
    // rejection observer now so a fast physical I/O failure is never reported
    // as an unhandled promise rejection.
    void promise.catch(() => undefined);

    const entry: PendingAcknowledgement = {
      operation: request.operation,
      path: request.path,
      promise,
      resolve,
      reject,
      settled: false,
    };
    entry.timer = setTimeout(() => {
      this.settleFailure(
        request.messageId,
        `Timed out after ${this.timeoutMs}ms waiting for ${request.operation} acknowledgement for '${request.path}'`,
      );
    }, this.timeoutMs);
    this.pending.set(request.messageId, entry);
  }

  acknowledge(messageId: string, error: string | null): boolean {
    const entry = this.pending.get(messageId);
    if (!entry) return false;

    if (error) {
      this.settleFailure(
        messageId,
        `Physical ${entry.operation} failed for '${entry.path}': ${error}`,
      );
    } else {
      this.settleSuccess(entry);
    }
    return true;
  }

  async waitForPending(): Promise<void> {
    const failures: Error[] = [];

    while (this.pending.size > 0) {
      const snapshot = Array.from(this.pending.entries());
      const results = await Promise.allSettled(snapshot.map(([, entry]) => entry.promise));
      results.forEach((result) => {
        if (result.status === 'rejected') {
          failures.push(
            result.reason instanceof Error ? result.reason : new Error(String(result.reason)),
          );
        }
      });
      snapshot.forEach(([messageId, entry]) => {
        if (this.pending.get(messageId) === entry) {
          if (entry.timer) clearTimeout(entry.timer);
          this.pending.delete(messageId);
        }
      });
    }

    if (failures.length === 1) throw failures[0];
    if (failures.length > 1) {
      throw new Error(
        `Multiple batch worker filesystem operations failed: ${failures
          .map((failure) => failure.message)
          .join('; ')}`,
      );
    }
  }

  private settleSuccess(entry: PendingAcknowledgement): void {
    if (entry.settled) return;
    entry.settled = true;
    if (entry.timer) clearTimeout(entry.timer);
    entry.resolve();
  }

  private settleFailure(messageId: string, message: string): void {
    const entry = this.pending.get(messageId);
    if (!entry || entry.settled) return;
    entry.settled = true;
    if (entry.timer) clearTimeout(entry.timer);
    entry.reject(new Error(message));
  }
}

export class OrderedBatchWorkerFileSystemQueue {
  private tail: Promise<void> = Promise.resolve();

  enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.tail.then(operation);
    this.tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  waitForIdle(): Promise<void> {
    return this.tail;
  }
}

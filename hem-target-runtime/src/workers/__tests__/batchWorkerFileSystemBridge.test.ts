// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  BatchWorkerFileSystemAcknowledgements,
  OrderedBatchWorkerFileSystemQueue,
  deleteBatchWorkerFile,
  deleteBatchWorkerWorkspaceFile,
} from '../batchWorkerFileSystemBridge';
import { FS_REQUEST_TYPE, isFileSystemRequestMessage } from '../workerProtocol';

const deleteWorkspaceFile = vi.fn();
const workspaceFileExists = vi.fn();

describe('batch worker filesystem bridge', () => {
  beforeEach(() => {
    deleteWorkspaceFile.mockReset().mockResolvedValue(undefined);
    workspaceFileExists.mockReset().mockResolvedValue(true);
  });

  it('invalidates the synchronous worker caches and emits a typed workspace delete request', () => {
    const path = 'output/demo/result_demo_demo_0/demo_sap.xml';
    const fileCache = new Map([[path, '<SAP-Report />']]);
    const fileExistsCache = new Map([[path, true]]);
    const postMessage = vi.fn();

    const result = deleteBatchWorkerFile(path, {
      fileCache,
      fileExistsCache,
      postMessage,
      createMessageId: () => 'delete_test_1',
    });

    expect(result).toEqual({ ok: true });
    expect(fileCache.has(path)).toBe(false);
    expect(fileExistsCache.get(path)).toBe(false);
    expect(postMessage).toHaveBeenCalledWith({
      type: FS_REQUEST_TYPE,
      operation: 'delete_file',
      path,
      messageId: 'delete_test_1',
    });
    expect(isFileSystemRequestMessage(postMessage.mock.calls[0][0])).toBe(true);
  });

  it('routes the delete request to the configured workspace', async () => {
    const path = 'output/demo/result_demo_demo_0/demo_sap.xml';
    const result = await deleteBatchWorkerWorkspaceFile(path, {
      deleteFile: deleteWorkspaceFile,
      fileExists: workspaceFileExists,
    });

    expect(workspaceFileExists).toHaveBeenCalledWith(path);
    expect(deleteWorkspaceFile).toHaveBeenCalledWith(path);
    expect(result).toEqual({ success: true });
  });

  it('treats an already-absent workspace file as an acknowledged deletion', async () => {
    const path = 'output/demo/result_demo_demo_0/demo_sap.xml';
    workspaceFileExists.mockResolvedValue(false);

    const result = await deleteBatchWorkerWorkspaceFile(path, {
      deleteFile: deleteWorkspaceFile,
      fileExists: workspaceFileExists,
    });

    expect(deleteWorkspaceFile).not.toHaveBeenCalled();
    expect(result).toEqual({ success: true });
  });

  it('does not finish draining until every write and delete is acknowledged', async () => {
    const acknowledgements = new BatchWorkerFileSystemAcknowledgements(1_000);
    acknowledgements.register({
      type: FS_REQUEST_TYPE,
      operation: 'delete_file',
      path: 'output/demo/old_sap.xml',
      messageId: 'delete_1',
    });
    acknowledgements.register({
      type: FS_REQUEST_TYPE,
      operation: 'write_file',
      path: 'output/demo/new_sap.xml',
      content: '<SAP-Report />',
      messageId: 'write_1',
    });

    let drained = false;
    const drain = acknowledgements.waitForPending().then(() => {
      drained = true;
    });

    acknowledgements.acknowledge('delete_1', null);
    await Promise.resolve();
    expect(drained).toBe(false);

    acknowledgements.acknowledge('write_1', null);
    await drain;
    expect(drained).toBe(true);
    expect(acknowledgements.pendingCount).toBe(0);
  });

  it('rejects the drain when physical workspace I/O fails', async () => {
    const acknowledgements = new BatchWorkerFileSystemAcknowledgements(1_000);
    acknowledgements.register({
      type: FS_REQUEST_TYPE,
      operation: 'delete_file',
      path: 'output/demo/stale_sap.xml',
      messageId: 'delete_1',
    });

    const drain = acknowledgements.waitForPending();
    acknowledgements.acknowledge('delete_1', 'permission denied');

    await expect(drain).rejects.toThrow(/delete_file.*permission denied/);
    expect(acknowledgements.pendingCount).toBe(0);
  });

  it('times out and cleans up a missing workspace acknowledgement', async () => {
    vi.useFakeTimers();
    try {
      const acknowledgements = new BatchWorkerFileSystemAcknowledgements(50);
      acknowledgements.register({
        type: FS_REQUEST_TYPE,
        operation: 'write_file',
        path: 'output/demo/sap_xml_status.json',
        content: '{"status":"disabled"}',
        messageId: 'write_1',
      });

      const rejection = expect(acknowledgements.waitForPending()).rejects.toThrow(
        /Timed out.*write_file.*sap_xml_status\.json/,
      );
      await vi.advanceTimersByTimeAsync(51);
      await rejection;
      expect(acknowledgements.pendingCount).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it('serializes physical workspace operations in worker-message arrival order', async () => {
    const queue = new OrderedBatchWorkerFileSystemQueue();
    const events: string[] = [];
    let releaseDelete!: () => void;
    const deleteGate = new Promise<void>((resolve) => {
      releaseDelete = resolve;
    });

    const deleting = queue.enqueue(async () => {
      events.push('delete-start');
      await deleteGate;
      events.push('delete-end');
    });
    const writing = queue.enqueue(async () => {
      events.push('write-start');
      events.push('write-end');
    });
    let idle = false;
    const waitingForIdle = queue.waitForIdle().then(() => {
      idle = true;
    });

    await Promise.resolve();
    expect(events).toEqual(['delete-start']);
    expect(idle).toBe(false);
    releaseDelete();
    await Promise.all([deleting, writing, waitingForIdle]);
    expect(events).toEqual(['delete-start', 'delete-end', 'write-start', 'write-end']);
    expect(idle).toBe(true);
  });
});

import type { DagResponse, TaskDetail, BasicTask, BatchSummary, StopBatchResponse } from './types';

export async function fetchDag(batchId: string): Promise<DagResponse> {
  const resp = await fetch(`/dag/${encodeURIComponent(batchId)}`);
  if (!resp.ok) {
    throw new Error(`HTTP ${resp.status}: ${resp.statusText}`);
  }
  return resp.json();
}

export async function fetchTask(taskId: string): Promise<TaskDetail> {
  const resp = await fetch(`/task/${encodeURIComponent(taskId)}`);
  if (!resp.ok) {
    throw new Error(`HTTP ${resp.status}: ${resp.statusText}`);
  }
  return resp.json();
}

export async function listTasks(
  page = 0,
  pageSize = 50,
): Promise<BasicTask[]> {
  const resp = await fetch(`/task?page=${page}&page_size=${pageSize}`);
  if (!resp.ok) {
    throw new Error(`HTTP ${resp.status}: ${resp.statusText}`);
  }
  return resp.json();
}

export async function stopBatch(batchId: string): Promise<StopBatchResponse> {
  const resp = await fetch(`/batch/${encodeURIComponent(batchId)}`, {
    method: 'DELETE',
  });
  if (!resp.ok) {
    const body = await resp.text();
    throw new Error(`HTTP ${resp.status}: ${body || resp.statusText}`);
  }
  return resp.json();
}

export async function listBatches(
  page = 0,
  pageSize = 20,
  filters?: { kind?: string; status?: string; name?: string },
): Promise<BatchSummary[]> {
  const params = new URLSearchParams({
    page: String(page),
    page_size: String(pageSize),
  });
  if (filters?.kind) params.set('kind', filters.kind);
  if (filters?.status) params.set('status', filters.status);
  if (filters?.name) params.set('name', filters.name);

  const resp = await fetch(`/batches?${params}`);
  if (!resp.ok) {
    throw new Error(`HTTP ${resp.status}: ${resp.statusText}`);
  }
  return resp.json();
}

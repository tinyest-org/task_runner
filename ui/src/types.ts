export type TaskStatus =
  | 'Waiting'
  | 'Pending'
  | 'Claimed'
  | 'Running'
  | 'Success'
  | 'Failure'
  | 'Paused'
  | 'Canceled';

export interface BasicTask {
  id: string;
  name: string;
  kind: string;
  status: TaskStatus;
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  success: number;
  failures: number;
  batch_id: string | null;
}

export interface TaskDetail extends BasicTask {
  timeout: number;
  metadata: Record<string, unknown>;
  failure_reason: string | null;
  last_updated: string;
  rules: unknown[];
  actions: ActionDto[];
}

export interface ActionDto {
  kind: string;
  trigger: string;
  params: Record<string, unknown>;
}

export interface Link {
  parent_id: string;
  child_id: string;
  requires_success: boolean;
}

export interface DagResponse {
  tasks: BasicTask[];
  links: Link[];
}

export interface BatchStatusCounts {
  waiting: number;
  pending: number;
  claimed: number;
  running: number;
  success: number;
  failure: number;
  paused: number;
  canceled: number;
}

export interface StopBatchResponse {
  batch_id: string;
  canceled_waiting: number;
  canceled_pending: number;
  canceled_claimed: number;
  canceled_running: number;
  canceled_paused: number;
  already_terminal: number;
}

export interface BatchSummary {
  batch_id: string;
  total_tasks: number;
  first_created_at: string;
  latest_updated_at: string;
  status_counts: BatchStatusCounts;
  kinds: string[];
}

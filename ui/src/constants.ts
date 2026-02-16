import type { TaskStatus } from './types';

export const STATUS_COLORS: Record<TaskStatus, string> = {
  Waiting: '#666666',
  Pending: '#f39c12',
  Running: '#3498db',
  Success: '#27ae60',
  Failure: '#e74c3c',
  Paused: '#9b59b6',
  Canceled: '#95a5a6',
};

export const STATUS_BG_CLASSES: Record<TaskStatus, string> = {
  Waiting: 'bg-gray-500',
  Pending: 'bg-amber-500',
  Running: 'bg-blue-500',
  Success: 'bg-emerald-500',
  Failure: 'bg-red-500',
  Paused: 'bg-purple-500',
  Canceled: 'bg-gray-400',
};

export const ALL_STATUSES: TaskStatus[] = [
  'Waiting',
  'Pending',
  'Running',
  'Success',
  'Failure',
  'Paused',
  'Canceled',
];

export const AUTO_REFRESH_INTERVAL = 5000;

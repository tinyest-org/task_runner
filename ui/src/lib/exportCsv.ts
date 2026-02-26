import type { BasicTask } from '../types';

const HEADERS = [
  'id',
  'name',
  'kind',
  'status',
  'created_at',
  'started_at',
  'ended_at',
  'success',
  'failures',
  'expected_count',
  'batch_id',
] as const;

function escapeCsv(value: string): string {
  if (value.includes(',') || value.includes('"') || value.includes('\n') || value.includes('\r')) {
    return `"${value.replace(/"/g, '""')}"`;
  }
  return value;
}

export function exportTasksCsv(tasks: BasicTask[], filename?: string): void {
  const rows = [HEADERS.join(',')];
  for (const t of tasks) {
    rows.push(
      [
        escapeCsv(t.id),
        escapeCsv(t.name),
        escapeCsv(t.kind),
        t.status,
        t.created_at,
        t.started_at ?? '',
        t.ended_at ?? '',
        String(t.success),
        String(t.failures),
        t.expected_count != null ? String(t.expected_count) : '',
        t.batch_id ?? '',
      ].join(','),
    );
  }

  const blob = new Blob([rows.join('\r\n')], { type: 'text/csv;charset=utf-8;' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename ?? `tasks-${new Date().toISOString().slice(0, 19).replace(/:/g, '-')}.csv`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

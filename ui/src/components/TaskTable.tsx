import { createSignal, For, Show } from 'solid-js';
import type { DagResponse, BasicTask } from '../types';
import { STATUS_BG_CLASSES } from '../constants';

interface Props {
  data: DagResponse | null;
  onTaskClick: (task: BasicTask) => void;
}

type SortKey = 'name' | 'kind' | 'status' | 'created_at' | 'started_at' | 'ended_at' | 'success' | 'failures';

const COLUMNS: { key: SortKey; label: string }[] = [
  { key: 'name', label: 'Name' },
  { key: 'kind', label: 'Kind' },
  { key: 'status', label: 'Status' },
  { key: 'created_at', label: 'Created' },
  { key: 'started_at', label: 'Started' },
  { key: 'ended_at', label: 'Ended' },
  { key: 'success', label: 'Success' },
  { key: 'failures', label: 'Failures' },
];

function compare(a: BasicTask, b: BasicTask, key: SortKey): number {
  const av = a[key];
  const bv = b[key];
  if (av == null && bv == null) return 0;
  if (av == null) return 1;
  if (bv == null) return -1;
  if (typeof av === 'number' && typeof bv === 'number') return av - bv;
  return String(av).localeCompare(String(bv));
}

function formatDate(d: string | null): string {
  if (!d) return '—';
  const date = new Date(d);
  return date.toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

export default function TaskTable(props: Props) {
  const [sortKey, setSortKey] = createSignal<SortKey>('name');
  const [sortAsc, setSortAsc] = createSignal(true);

  function handleSort(key: SortKey) {
    if (sortKey() === key) {
      setSortAsc((v) => !v);
    } else {
      setSortKey(key);
      setSortAsc(true);
    }
  }

  const sortedTasks = () => {
    const tasks = props.data?.tasks;
    if (!tasks) return [];
    const key = sortKey();
    const asc = sortAsc();
    return [...tasks].sort((a, b) => {
      const c = compare(a, b, key);
      return asc ? c : -c;
    });
  };

  return (
    <div class="flex-1 overflow-auto p-4">
      <Show when={props.data} fallback={
        <div class="flex h-full items-center justify-center text-white/40">
          No data loaded
        </div>
      }>
        <table class="w-full border-collapse text-sm text-white/90">
          <thead>
            <tr class="border-b border-white/10">
              <For each={COLUMNS}>
                {(col) => (
                  <th
                    class="cursor-pointer select-none px-3 py-2 text-left text-xs font-semibold uppercase text-white/50 hover:text-white/80"
                    onClick={() => handleSort(col.key)}
                  >
                    {col.label}
                    <Show when={sortKey() === col.key}>
                      <span class="ml-1">{sortAsc() ? '▲' : '▼'}</span>
                    </Show>
                  </th>
                )}
              </For>
            </tr>
          </thead>
          <tbody>
            <For each={sortedTasks()}>
              {(task) => (
                <tr
                  class="cursor-pointer border-b border-white/5 transition-colors hover:bg-white/5"
                  onClick={() => props.onTaskClick(task)}
                >
                  <td class="max-w-[240px] truncate px-3 py-2 font-medium">{task.name}</td>
                  <td class="px-3 py-2 text-white/60">{task.kind}</td>
                  <td class="px-3 py-2">
                    <span
                      class={`inline-block rounded px-2 py-0.5 text-xs font-semibold uppercase text-white ${STATUS_BG_CLASSES[task.status] ?? 'bg-gray-500'}`}
                    >
                      {task.status}
                    </span>
                  </td>
                  <td class="px-3 py-2 text-white/50">{formatDate(task.created_at)}</td>
                  <td class="px-3 py-2 text-white/50">{formatDate(task.started_at)}</td>
                  <td class="px-3 py-2 text-white/50">{formatDate(task.ended_at)}</td>
                  <td class="px-3 py-2 text-center">{task.success}</td>
                  <td class="px-3 py-2 text-center">{task.failures}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </div>
  );
}

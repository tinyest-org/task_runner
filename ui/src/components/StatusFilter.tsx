import { For } from 'solid-js';
import type { BasicTask, TaskStatus } from '../types';
import { ALL_STATUSES, STATUS_COLORS } from '../constants';

interface Props {
  tasks: BasicTask[];
  activeFilters: Set<TaskStatus>;
  onToggle: (status: TaskStatus) => void;
}

export default function StatusFilter(props: Props) {
  const counts = () => {
    const map: Partial<Record<TaskStatus, number>> = {};
    for (const t of props.tasks) {
      map[t.status] = (map[t.status] ?? 0) + 1;
    }
    return map;
  };

  const visibleStatuses = () =>
    ALL_STATUSES.filter((s) => (counts()[s] ?? 0) > 0);

  return (
    <div class="flex flex-wrap gap-1.5">
      <For each={visibleStatuses()}>
        {(status) => {
          const active = () => props.activeFilters.has(status);
          const color = STATUS_COLORS[status];
          return (
            <button
              class="flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium transition-colors"
              classList={{
                'border-white/20 bg-white/5 text-white/60 hover:bg-white/10': !active(),
                'border-white/40 text-white': active(),
              }}
              style={active() ? { 'background-color': `${color}33` } : undefined}
              onClick={() => props.onToggle(status)}
            >
              <span
                class="inline-block h-2 w-2 rounded-full"
                style={{ 'background-color': color }}
              />
              {status}
              <span class="text-white/40">{counts()[status]}</span>
            </button>
          );
        }}
      </For>
    </div>
  );
}

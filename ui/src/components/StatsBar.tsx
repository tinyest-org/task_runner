import { Show } from 'solid-js';
import type { BasicTask, TaskStatus } from '../types';

interface Props {
  tasks: BasicTask[];
  linkCount: number;
}

export default function StatsBar(props: Props) {
  const statusCounts = () => {
    const counts: Partial<Record<TaskStatus, number>> = {};
    for (const t of props.tasks) {
      counts[t.status] = (counts[t.status] ?? 0) + 1;
    }
    return Object.entries(counts)
      .map(([s, c]) => `${s}: ${c}`)
      .join(' | ');
  };

  return (
    <span class="text-xs text-white/50">
      <Show when={props.tasks.length > 0}>
        {props.tasks.length} tasks, {props.linkCount} links | {statusCounts()}
      </Show>
    </span>
  );
}

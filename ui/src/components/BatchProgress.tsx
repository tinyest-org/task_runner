import { Show } from 'solid-js';
import type { BasicTask } from '../types';

interface Props {
  tasks: BasicTask[];
}

export default function BatchProgress(props: Props) {
  const stats = () => {
    const total = props.tasks.length;
    if (total === 0) return null;

    let success = 0;
    let failure = 0;
    let canceled = 0;
    let running = 0;
    let pending = 0;

    for (const t of props.tasks) {
      switch (t.status) {
        case 'Success':
          success++;
          break;
        case 'Failure':
          failure++;
          break;
        case 'Canceled':
          canceled++;
          break;
        case 'Running':
        case 'Claimed':
          running++;
          break;
        case 'Pending':
          pending++;
          break;
        // Waiting + Paused = remainder (shown as dark gap)
      }
    }

    const completed = success + failure + canceled;
    const pct = Math.round((completed / total) * 100);

    return { total, success, failure, canceled, running, pending, pct };
  };

  return (
    <Show when={stats()}>
      {(s) => (
        <div class="flex items-center gap-2">
          <div class="flex h-1.5 w-28 overflow-hidden rounded-full bg-white/10">
            <div
              class="h-full bg-emerald-500 transition-all duration-500"
              style={{ width: `${(s().success / s().total) * 100}%` }}
            />
            <div
              class="h-full bg-red-500 transition-all duration-500"
              style={{ width: `${(s().failure / s().total) * 100}%` }}
            />
            <div
              class="h-full bg-gray-400 transition-all duration-500"
              style={{ width: `${(s().canceled / s().total) * 100}%` }}
            />
            <div
              class="h-full bg-blue-500 transition-all duration-500"
              style={{ width: `${(s().running / s().total) * 100}%` }}
            />
            <div
              class="h-full bg-amber-500 transition-all duration-500"
              style={{ width: `${(s().pending / s().total) * 100}%` }}
            />
          </div>
          <span class="text-xs tabular-nums text-white/50">{s().pct}%</span>
        </div>
      )}
    </Show>
  );
}

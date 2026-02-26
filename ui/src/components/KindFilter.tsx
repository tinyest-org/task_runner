import { For } from 'solid-js';
import type { BasicTask } from '../types';

interface Props {
  tasks: BasicTask[];
  activeKinds: Set<string>;
  onToggle: (kind: string) => void;
}

export default function KindFilter(props: Props) {
  const kinds = () => {
    const map = new Map<string, number>();
    for (const t of props.tasks) {
      map.set(t.kind, (map.get(t.kind) ?? 0) + 1);
    }
    return Array.from(map.entries()).sort((a, b) => a[0].localeCompare(b[0]));
  };

  return (
    <div class="flex flex-wrap gap-1.5">
      <For each={kinds()}>
        {([kind, count]) => {
          const active = () => props.activeKinds.has(kind);
          return (
            <button
              class="flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium transition-colors"
              classList={{
                'border-white/20 bg-white/5 text-white/60 hover:bg-white/10': !active(),
                'border-cyan-400/40 bg-cyan-400/15 text-cyan-300': active(),
              }}
              onClick={() => props.onToggle(kind)}
            >
              {kind}
              <span class="text-white/40">{count}</span>
            </button>
          );
        }}
      </For>
    </div>
  );
}

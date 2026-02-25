import { For } from 'solid-js';
import { ALL_STATUSES, STATUS_COLORS } from '../constants';
import { Card } from 'glass-ui-solid';

export default function Legend() {
  return (
    <Card class="fixed bottom-5 left-5 z-20 space-y-1 px-4 py-3 text-xs">
      <For each={ALL_STATUSES}>
        {(status) => (
          <div class="flex items-center gap-2">
            <div
              class="h-3.5 w-3.5 rounded-sm"
              style={{ background: STATUS_COLORS[status] }}
            />
            <span class="text-white/80">{status}</span>
          </div>
        )}
      </For>
      <div class="mt-1 border-t border-white/10 pt-1">
        <div class="flex items-center gap-2">
          <div
            class="h-3.5 w-3.5 rounded-sm border-2 border-dashed border-amber-400 bg-transparent"
          />
          <span class="text-white/80">Dead-end barrier</span>
        </div>
      </div>
    </Card>
  );
}

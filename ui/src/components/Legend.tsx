import { createSignal, For, Show } from 'solid-js';
import { ALL_STATUSES, STATUS_COLORS } from '../constants';
import { Card } from 'glass-ui-solid';

export default function Legend() {
  const [collapsed, setCollapsed] = createSignal(window.innerWidth < 640);

  return (
    <Card class="fixed bottom-5 left-5 z-20 px-3 py-2 text-xs">
      <button
        class="flex w-full items-center justify-between gap-2 text-white/60 hover:text-white/80"
        onClick={() => setCollapsed((v) => !v)}
      >
        <span class="font-medium">Legend</span>
        <span class="text-[10px]">{collapsed() ? '\u25B6' : '\u25BC'}</span>
      </button>
      <Show when={!collapsed()}>
        <div class="mt-1.5 space-y-1">
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
        </div>
      </Show>
    </Card>
  );
}

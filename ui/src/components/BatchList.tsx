import { Show, For, createSignal, createEffect, on } from 'solid-js';
import { listBatches } from '../api';
import { getRecentBatches } from '../storage';
import type { BatchSummary, TaskStatus } from '../types';
import { STATUS_BG_CLASSES } from '../constants';
import { Drawer, Input, Button } from 'glass-ui-solid';

interface Props {
  open: boolean;
  onClose: () => void;
  onSelect: (batchId: string) => void;
}

const STATUS_ORDER: TaskStatus[] = [
  'Running',
  'Claimed',
  'Pending',
  'Waiting',
  'Success',
  'Failure',
  'Paused',
  'Canceled',
];

function timeAgo(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime();
  const secs = Math.floor(diff / 1000);
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

export default function BatchList(props: Props) {
  const [batches, setBatches] = createSignal<BatchSummary[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [search, setSearch] = createSignal('');
  const [page, setPage] = createSignal(0);
  const [hasMore, setHasMore] = createSignal(false);
  const [recents, setRecents] = createSignal<string[]>([]);

  let debounceTimer: ReturnType<typeof setTimeout> | undefined;
  const PAGE_SIZE = 20;

  const load = async (pageNum = 0, append = false) => {
    setLoading(true);
    setError(null);
    try {
      const query = search().trim();
      const filters = query ? { name: query } : undefined;
      const results = await listBatches(pageNum, PAGE_SIZE, filters);
      if (append) {
        setBatches((prev) => [...prev, ...results]);
      } else {
        setBatches(results);
      }
      setHasMore(results.length === PAGE_SIZE);
      setPage(pageNum);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load batches');
    } finally {
      setLoading(false);
    }
  };

  const loadMore = () => {
    load(page() + 1, true);
  };

  // Load when drawer opens
  createEffect(
    on(
      () => props.open,
      (open, prevOpen) => {
        if (open && !prevOpen) {
          setRecents(getRecentBatches());
          setSearch('');
          load(0);
        }
      },
    ),
  );

  // Debounced search
  createEffect(
    on(
      () => search(),
      () => {
        if (!props.open) return;
        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => load(0), 300);
      },
      { defer: true },
    ),
  );

  const showRecents = () => search().trim() === '' && recents().length > 0;

  return (
    <Drawer
      open={props.open}
      onClose={props.onClose}
      title="Batches"
      position="left"
      size="md"
    >
      {/* Search */}
      <Input
        value={search()}
        onInput={setSearch}
        placeholder="Search by name..."
        size="sm"
        class="mb-3 w-full"
      />

      <Show when={error()}>
        <div class="mb-2 text-xs text-red-400">{error()}</div>
      </Show>

      <div class="flex-1 space-y-1.5 overflow-y-auto">
        {/* Recently Viewed */}
        <Show when={showRecents()}>
          <div class="mb-2">
            <div class="mb-1 text-[10px] font-medium uppercase tracking-wider text-white/40">
              Recently Viewed
            </div>
            <For each={recents()}>
              {(bid) => (
                <button
                  class="w-full cursor-pointer rounded px-2 py-1 text-left font-mono text-xs text-white/70 hover:bg-white/10"
                  onClick={() => {
                    props.onSelect(bid);
                    props.onClose();
                  }}
                >
                  {bid.substring(0, 8)}...
                </button>
              )}
            </For>
            <div class="my-2 border-t border-white/10" />
          </div>
        </Show>

        {/* Loading state (initial) */}
        <Show when={loading() && batches().length === 0}>
          <div class="text-center text-xs text-white/50">Loading...</div>
        </Show>

        {/* Batch cards */}
        <Show
          when={batches().length > 0}
          fallback={
            <Show when={!loading()}>
              <div class="text-xs text-white/40">No batches found</div>
            </Show>
          }
        >
          <For each={batches()}>
            {(batch) => (
              <button
                class="w-full cursor-pointer rounded-lg border border-white/5 bg-white/5 px-3 py-2 text-left transition hover:bg-white/10"
                onClick={() => {
                  props.onSelect(batch.batch_id);
                  props.onClose();
                }}
              >
                {/* Header: truncated ID + time */}
                <div class="flex items-center justify-between">
                  <span class="font-mono text-xs text-white/80">
                    {batch.batch_id.substring(0, 8)}...
                  </span>
                  <span class="text-[10px] text-white/40">
                    {timeAgo(batch.first_created_at)}
                  </span>
                </div>

                {/* Tasks count + kinds */}
                <div class="mt-1 flex items-center gap-2">
                  <span class="text-[10px] text-white/50">
                    {batch.total_tasks} task{batch.total_tasks !== 1 ? 's' : ''}
                  </span>
                  <For each={batch.kinds.slice(0, 3)}>
                    {(kind) => (
                      <span class="rounded bg-white/10 px-1 py-0.5 text-[10px] text-white/60">
                        {kind}
                      </span>
                    )}
                  </For>
                  <Show when={batch.kinds.length > 3}>
                    <span class="text-[10px] text-white/40">
                      +{batch.kinds.length - 3}
                    </span>
                  </Show>
                </div>

                {/* Status mini-badges */}
                <div class="mt-1.5 flex flex-wrap gap-1">
                  <For each={STATUS_ORDER}>
                    {(status) => {
                      const key = status.toLowerCase() as keyof typeof batch.status_counts;
                      const count = batch.status_counts[key];
                      return (
                        <Show when={count > 0}>
                          <span
                            class={`rounded px-1 py-0.5 text-[10px] font-medium text-white ${STATUS_BG_CLASSES[status]}`}
                          >
                            {count} {status}
                          </span>
                        </Show>
                      );
                    }}
                  </For>
                </div>
              </button>
            )}
          </For>
        </Show>

        {/* Load more */}
        <Show when={hasMore() && !loading()}>
          <Button
            variant="ghost"
            size="sm"
            fullWidth
            onClick={loadMore}
          >
            Load more...
          </Button>
        </Show>

        {/* Loading indicator for pagination */}
        <Show when={loading() && batches().length > 0}>
          <div class="mt-2 text-center text-xs text-white/50">Loading...</div>
        </Show>
      </div>
    </Drawer>
  );
}

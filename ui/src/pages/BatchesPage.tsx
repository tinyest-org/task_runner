import { createSignal, createEffect, on, onCleanup, For, Show, onMount } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { listBatches } from '../api';
import { getRecentBatches } from '../storage';
import type { BatchSummary, TaskStatus } from '../types';
import { STATUS_BG_CLASSES } from '../constants';
import { Input, Button } from 'glass-ui-solid';
import { useTheme } from '../App';

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

export default function BatchesPage() {
  const navigate = useNavigate();
  const { theme, toggle: toggleTheme } = useTheme();
  const [batches, setBatches] = createSignal<BatchSummary[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [search, setSearch] = createSignal('');
  const [page, setPage] = createSignal(0);
  const [hasMore, setHasMore] = createSignal(false);
  const [directId, setDirectId] = createSignal('');

  let debounceTimer: ReturnType<typeof setTimeout> | undefined;
  const PAGE_SIZE = 20;

  const recents = () => getRecentBatches();

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

  onMount(() => load(0));
  onCleanup(() => clearTimeout(debounceTimer));

  // Debounced search
  createEffect(
    on(
      () => search(),
      () => {
        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => load(0), 300);
      },
      { defer: true },
    ),
  );

  function goToBatch(batchId: string) {
    navigate(`/batch/${batchId}`);
  }

  function handleDirectGo() {
    const id = directId().trim();
    if (id) goToBatch(id);
  }

  return (
    <div class="flex flex-1 flex-col overflow-hidden">
      <header class="glass-navbar relative z-10 flex items-center gap-3 px-5 py-3">
        <h1 class="text-lg font-medium" style={{ color: 'var(--accent)' }}>ArcRun</h1>
        <div class="ml-auto">
          <button
            title={`Switch to ${theme() === 'dark' ? 'light' : 'dark'} mode`}
            class="theme-btn rounded-md border px-2 py-1 text-xs transition-colors hover:opacity-80"
            onClick={toggleTheme}
          >
            {theme() === 'dark' ? 'Light' : 'Dark'}
          </button>
        </div>
      </header>

      <div class="flex-1 overflow-y-auto px-5 py-4">
        {/* Direct batch ID input */}
        <div class="mb-6 flex flex-col gap-2 sm:flex-row sm:items-center">
          <Input
            value={directId()}
            onInput={setDirectId}
            onKeyDown={(e) => {
              if (e.key === 'Enter') handleDirectGo();
            }}
            placeholder="Paste a batch ID to go directly..."
            size="sm"
            class="w-full sm:w-96"
          />
          <Button
            variant="primary"
            size="sm"
            onClick={handleDirectGo}
            disabled={!directId().trim()}
          >
            Go
          </Button>
        </div>

        {/* Recent batches */}
        <Show when={recents().length > 0 && !search().trim()}>
          <div class="mb-6">
            <h2 class="mb-2 text-xs font-medium uppercase tracking-wider text-white/40">
              Recently Viewed
            </h2>
            <div class="flex flex-wrap gap-2">
              <For each={recents()}>
                {(bid) => (
                  <button
                    class="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 font-mono text-xs text-white/70 transition hover:bg-white/10"
                    onClick={() => goToBatch(bid)}
                  >
                    {bid.substring(0, 8)}...
                  </button>
                )}
              </For>
            </div>
          </div>
        </Show>

        {/* Search */}
        <div class="mb-4">
          <h2 class="mb-2 text-xs font-medium uppercase tracking-wider text-white/40">
            All Batches
          </h2>
          <Input
            value={search()}
            onInput={setSearch}
            placeholder="Search by name..."
            size="sm"
            class="w-full sm:w-96"
          />
        </div>

        <Show when={error()}>
          <div class="mb-2 text-xs text-red-400">{error()}</div>
        </Show>

        <Show when={loading() && batches().length === 0}>
          <div class="text-sm text-white/50">Loading...</div>
        </Show>

        {/* Batch grid */}
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
          <For each={batches()}>
            {(batch) => (
              <button
                class="w-full cursor-pointer rounded-lg border border-white/10 bg-white/5 px-4 py-3 text-left transition hover:border-white/20 hover:bg-white/10"
                onClick={() => goToBatch(batch.batch_id)}
              >
                <div class="flex items-center justify-between">
                  <span class="font-mono text-xs text-white/80">
                    {batch.batch_id.substring(0, 12)}...
                  </span>
                  <span class="text-[10px] text-white/40">
                    {timeAgo(batch.first_created_at)}
                  </span>
                </div>
                <div class="mt-1.5 flex items-center gap-2">
                  <span class="text-xs text-white/50">
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
                    <span class="text-[10px] text-white/40">+{batch.kinds.length - 3}</span>
                  </Show>
                </div>
                <div class="mt-2 flex flex-wrap gap-1">
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
        </div>

        <Show when={batches().length === 0 && !loading()}>
          <div class="mt-4 text-sm text-white/40">No batches found</div>
        </Show>

        <Show when={hasMore() && !loading()}>
          <div class="mt-4 text-center">
            <Button variant="secondary" size="sm" onClick={() => load(page() + 1, true)}>
              Load more...
            </Button>
          </div>
        </Show>

        <Show when={loading() && batches().length > 0}>
          <div class="mt-4 text-center text-sm text-white/50">Loading...</div>
        </Show>
      </div>
    </div>
  );
}

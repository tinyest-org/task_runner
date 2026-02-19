import "glass-ui-solid/styles.css";
import "glass-ui-solid/theme.css";
import { createSignal, onCleanup, Show, For } from 'solid-js';
import { useSearchParams } from '@solidjs/router';
import type { DagResponse, BasicTask } from './types';
import { fetchDag } from './api';
import { addRecentBatch } from './storage';
import { AUTO_REFRESH_INTERVAL } from './constants';
import { Input, Button, Card, useWindowManager } from 'glass-ui-solid';
import DagCanvas, { type DagCanvasAPI } from './components/DagCanvas';
import IsometricView from './components/IsometricView';
import TaskInfoPanel from './components/TaskInfoPanel';
import BatchList from './components/BatchList';
import Legend from './components/Legend';
import StatsBar from './components/StatsBar';


export default function App() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [batchId, setBatchId] = createSignal('');
  const [dagData, setDagData] = createSignal<DagResponse | null>(null);
  const [openTaskIds, setOpenTaskIds] = createSignal<string[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [batchListOpen, setBatchListOpen] = createSignal(false);
  const [viewMode, setViewMode] = createSignal<'dag' | 'iso'>('iso');
  const [message, setMessage] = createSignal<string | null>(
    'Enter a batch ID to visualize the task DAG',
  );

  const windowManager = useWindowManager();
  const windowHandles = new Map<string, ReturnType<typeof windowManager.register>>();

  function getWindowHandle(taskId: string) {
    let handle = windowHandles.get(taskId);
    if (!handle) {
      handle = windowManager.register(taskId);
      windowHandles.set(taskId, handle);
    }
    return handle;
  }

  function removeWindowHandle(taskId: string) {
    const handle = windowHandles.get(taskId);
    if (handle) {
      handle.unregister();
      windowHandles.delete(taskId);
    }
  }

  function clearAllWindows() {
    for (const [, handle] of windowHandles) {
      handle.unregister();
    }
    windowHandles.clear();
    setOpenTaskIds([]);
  }

  let canvasApi: DagCanvasAPI | undefined;
  let refreshTimer: ReturnType<typeof setInterval> | undefined;

  function startRefreshTimer() {
    if (refreshTimer) {
      clearInterval(refreshTimer);
      refreshTimer = undefined;
    }
    if (batchId().trim()) {
      refreshTimer = setInterval(() => loadDag(), AUTO_REFRESH_INTERVAL);
    }
  }

  async function loadDag(id?: string) {
    const bid = id ?? batchId().trim();
    if (!bid) return;

    const isNewBatch = bid !== batchId();
    setBatchId(bid);
    setSearchParams({ batch: bid }, { replace: true });
    addRecentBatch(bid);

    if (isNewBatch) {
      clearAllWindows();
    }

    setLoading(true);
    setError(null);
    setMessage(null);

    try {
      const data = await fetchDag(bid);
      if (data.tasks.length === 0) {
        setMessage('No tasks found for this batch');
        setDagData(null);
        clearAllWindows();
      } else {
        setDagData(data);
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to load DAG';
      setError(msg);
      setMessage(`Error: ${msg}`);
    } finally {
      setLoading(false);
    }

    startRefreshTimer();
  }

  function openTask(task: BasicTask) {
    if (openTaskIds().includes(task.id)) {
      getWindowHandle(task.id).focus();
      return;
    }
    getWindowHandle(task.id);
    setOpenTaskIds((prev) => [...prev, task.id]);
  }

  function closeTask(taskId: string) {
    removeWindowHandle(taskId);
    setOpenTaskIds((prev) => prev.filter((id) => id !== taskId));
  }

  function getTask(taskId: string): BasicTask | undefined {
    return dagData()?.tasks.find((t) => t.id === taskId);
  }

  function handleBatchSelect(bid: string) {
    loadDag(bid);
  }

  onCleanup(() => {
    if (refreshTimer) clearInterval(refreshTimer);
  });

  // Load from URL on mount
  const initialBatch = typeof searchParams.batch === 'string' ? searchParams.batch : undefined;
  if (initialBatch) {
    loadDag(initialBatch);
  }

  return (
    <div class="flex h-screen flex-col bg-[#0a0a1a]">
      {/* Toolbar */}
      <header class="glass-navbar relative z-10 flex flex-wrap items-center gap-3 px-5 py-3">
        <h1 class="text-lg font-medium text-rose-400">Task DAG Viewer</h1>

        <div class="flex flex-wrap items-center gap-2">
          <Input
            value={batchId()}
            onInput={setBatchId}
            onKeyDown={(e) => { if (e.key === 'Enter') loadDag(); }}
            placeholder="Enter batch ID (UUID)..."
            size="sm"
            class="w-72"
          />
          <Button
            variant="primary"
            size="sm"
            onClick={() => loadDag()}
            disabled={loading()}
          >
            {loading() ? 'Loading...' : 'Load DAG'}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => setBatchListOpen(true)}
          >
            List Batches
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => setViewMode(v => v === 'dag' ? 'iso' : 'dag')}
            disabled={!dagData()}
          >
            {viewMode() === 'dag' ? '3D View' : '2D View'}
          </Button>
          <Show when={viewMode() === 'dag'}>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => canvasApi?.fit()}
              disabled={!dagData()}
            >
              Fit View
            </Button>
          </Show>
          <Show when={openTaskIds().length > 0}>
            <Button
              variant="secondary"
              size="sm"
              onClick={clearAllWindows}
            >
              Close All ({openTaskIds().length})
            </Button>
          </Show>

          <StatsBar
            tasks={dagData()?.tasks ?? []}
            linkCount={dagData()?.links.length ?? 0}
          />
        </div>
      </header>

      {/* Main content */}
      <div class="relative z-1 flex flex-1 overflow-hidden">
        <Show when={message() && !dagData()}>
          <div class="absolute inset-0 flex items-center justify-center text-white/40">
            {message()}
          </div>
        </Show>

        <Show when={viewMode() === 'dag'} fallback={
          <IsometricView data={dagData()} onNodeClick={openTask} onBackgroundClick={() => {}} />
        }>
          <DagCanvas
            data={dagData()}
            onNodeClick={(task) => openTask(task)}
            onBackgroundClick={() => {}}
            ref={(api) => (canvasApi = api)}
          />
        </Show>
      </div>

      <Legend />

      {/* Task detail windows */}
      <For each={openTaskIds()}>
        {(taskId, i) => {
          const handle = getWindowHandle(taskId);
          const task = () => getTask(taskId);
          return (
            <Show when={task()}>
              {(t) => (
                <TaskInfoPanel
                  task={t()}
                  index={i()}
                  onClose={() => closeTask(taskId)}
                  zIndex={handle.zIndex}
                  onFocus={handle.focus}
                />
              )}
            </Show>
          );
        }}
      </For>

      <BatchList
        open={batchListOpen()}
        onClose={() => setBatchListOpen(false)}
        onSelect={handleBatchSelect}
      />

      <Show when={error()}>
        <Card class="fixed bottom-5 right-5 z-50 border-red-500/40 px-4 py-3 text-sm text-red-300">
          {error()}
          <button
            aria-label="Dismiss error"
            class="ml-3 text-white/50 hover:text-white"
            onClick={() => setError(null)}
          >
            &times;
          </button>
        </Card>
      </Show>
    </div>
  );
}

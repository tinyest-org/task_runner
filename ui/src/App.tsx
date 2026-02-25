import "glass-ui-solid/styles.css";
import "glass-ui-solid/theme.css";
import { createSignal, onCleanup, Show, For, Switch, Match } from 'solid-js';
import { useSearchParams } from '@solidjs/router';
import type { DagResponse, BasicTask, StopBatchResponse, TaskStatus, UpdateBatchRulesResponse } from './types';
import { fetchDag, stopBatch } from './api';
import { addRecentBatch } from './storage';
import { AUTO_REFRESH_INTERVAL } from './constants';
import { Input, Button, Card, useWindowManager } from 'glass-ui-solid';
import DagCanvas, { type DagCanvasAPI } from './components/DagCanvas';
import IsometricView from './components/IsometricView';
import TaskInfoPanel from './components/TaskInfoPanel';
import BatchList from './components/BatchList';
import Legend from './components/Legend';
import StatsBar from './components/StatsBar';
import StatusFilter from './components/StatusFilter';
import TaskTable from './components/TaskTable';
import BatchRulesEditor from './components/BatchRulesEditor';


export default function App() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [batchId, setBatchId] = createSignal('');
  const [dagData, setDagData] = createSignal<DagResponse | null>(null);
  const [openTaskIds, setOpenTaskIds] = createSignal<string[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [batchListOpen, setBatchListOpen] = createSignal(false);
  const [viewMode, setViewMode] = createSignal<'dag' | 'iso' | 'table'>('iso');
  const [activeFilters, setActiveFilters] = createSignal<Set<TaskStatus>>(new Set());
  const [isoGroupBy, setIsoGroupBy] = createSignal<'dag' | 'status'>('dag');
  const [message, setMessage] = createSignal<string | null>(
    'Enter a batch ID to visualize the task DAG',
  );
  const [showCancelConfirm, setShowCancelConfirm] = createSignal(false);
  const [canceling, setCanceling] = createSignal(false);
  const [showRulesEditor, setShowRulesEditor] = createSignal(false);

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

  const filteredData = () => {
    const data = dagData();
    if (!data) return null;
    const filters = activeFilters();
    if (filters.size === 0) return data;
    const tasks = data.tasks.filter((t) => filters.has(t.status));
    const taskIds = new Set(tasks.map((t) => t.id));
    const links = data.links.filter((l) => taskIds.has(l.parent_id) && taskIds.has(l.child_id));
    return { tasks, links };
  };

  function toggleFilter(status: TaskStatus) {
    setActiveFilters((prev) => {
      const next = new Set(prev);
      if (next.has(status)) next.delete(status);
      else next.add(status);
      return next;
    });
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
      setActiveFilters(new Set());
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

  function hasActiveTasks(): boolean {
    const data = dagData();
    if (!data) return false;
    return data.tasks.some(
      (t) => !['Success', 'Failure', 'Canceled'].includes(t.status),
    );
  }

  async function handleCancelBatch() {
    const bid = batchId().trim();
    if (!bid) return;

    setCanceling(true);
    try {
      const result: StopBatchResponse = await stopBatch(bid);
      const total =
        result.canceled_waiting +
        result.canceled_pending +
        result.canceled_claimed +
        result.canceled_running +
        result.canceled_paused;
      setError(null);
      setMessage(`Batch stopped: ${total} task${total !== 1 ? 's' : ''} canceled`);
      // Refresh the DAG to show updated statuses
      await loadDag(bid);
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to cancel batch';
      setError(msg);
    } finally {
      setCanceling(false);
      setShowCancelConfirm(false);
    }
  }

  async function handleRulesUpdated(result: UpdateBatchRulesResponse) {
    setShowRulesEditor(false);
    setMessage(`Rules updated for kind "${result.kind}": ${result.updated_count} task${result.updated_count !== 1 ? 's' : ''} affected`);
    await loadDag(batchId());
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
          <div class="flex overflow-hidden rounded-md border border-white/20">
            <button
              class="px-2.5 py-1 text-xs font-medium transition-colors"
              classList={{
                'bg-white/15 text-white': viewMode() === 'dag',
                'text-white/50 hover:bg-white/5 hover:text-white/80': viewMode() !== 'dag',
              }}
              disabled={!dagData()}
              onClick={() => setViewMode('dag')}
            >
              2D
            </button>
            <button
              class="border-x border-white/20 px-2.5 py-1 text-xs font-medium transition-colors"
              classList={{
                'bg-white/15 text-white': viewMode() === 'iso',
                'text-white/50 hover:bg-white/5 hover:text-white/80': viewMode() !== 'iso',
              }}
              disabled={!dagData()}
              onClick={() => setViewMode('iso')}
            >
              3D
            </button>
            <button
              class="px-2.5 py-1 text-xs font-medium transition-colors"
              classList={{
                'bg-white/15 text-white': viewMode() === 'table',
                'text-white/50 hover:bg-white/5 hover:text-white/80': viewMode() !== 'table',
              }}
              disabled={!dagData()}
              onClick={() => setViewMode('table')}
            >
              Table
            </button>
          </div>
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
          <Show when={viewMode() === 'iso' && dagData()}>
            <div class="flex items-center gap-1">
              <span class="text-xs text-white/40">Group:</span>
              <div class="flex overflow-hidden rounded-md border border-white/20">
                <button
                  class="px-2 py-0.5 text-xs font-medium transition-colors"
                  classList={{
                    'bg-white/15 text-white': isoGroupBy() === 'dag',
                    'text-white/50 hover:bg-white/5 hover:text-white/80': isoGroupBy() !== 'dag',
                  }}
                  onClick={() => setIsoGroupBy('dag')}
                >
                  DAG
                </button>
                <button
                  class="border-l border-white/20 px-2 py-0.5 text-xs font-medium transition-colors"
                  classList={{
                    'bg-white/15 text-white': isoGroupBy() === 'status',
                    'text-white/50 hover:bg-white/5 hover:text-white/80': isoGroupBy() !== 'status',
                  }}
                  onClick={() => setIsoGroupBy('status')}
                >
                  Status
                </button>
              </div>
            </div>
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
          <Show when={dagData() && hasActiveTasks()}>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setShowRulesEditor(true)}
              class="!border-cyan-500/40 !text-cyan-400 hover:!bg-cyan-500/20"
            >
              Edit Rules
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setShowCancelConfirm(true)}
              class="!border-red-500/40 !text-red-400 hover:!bg-red-500/20"
            >
              Cancel Batch
            </Button>
          </Show>

          <StatsBar
            tasks={dagData()?.tasks ?? []}
            linkCount={dagData()?.links.length ?? 0}
          />
        </div>
        <Show when={dagData()}>
          <StatusFilter
            tasks={dagData()!.tasks}
            activeFilters={activeFilters()}
            onToggle={toggleFilter}
          />
        </Show>
      </header>

      {/* Main content */}
      <div class="relative z-1 flex flex-1 overflow-hidden">
        <Show when={message() && !dagData()}>
          <div class="absolute inset-0 flex items-center justify-center text-white/40">
            {message()}
          </div>
        </Show>

        <Switch>
          <Match when={viewMode() === 'dag'}>
            <DagCanvas
              data={filteredData()}
              onNodeClick={(task) => openTask(task)}
              onBackgroundClick={() => {}}
              ref={(api) => (canvasApi = api)}
            />
          </Match>
          <Match when={viewMode() === 'iso'}>
            <IsometricView data={filteredData()} onNodeClick={openTask} onBackgroundClick={() => {}} groupBy={isoGroupBy()} />
          </Match>
          <Match when={viewMode() === 'table'}>
            <TaskTable data={filteredData()} onTaskClick={openTask} />
          </Match>
        </Switch>
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
                  onCanceled={() => loadDag()}
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

      <BatchRulesEditor
        open={showRulesEditor()}
        batchId={batchId()}
        dagData={dagData()}
        onClose={() => setShowRulesEditor(false)}
        onUpdated={handleRulesUpdated}
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

      {/* Cancel batch confirmation modal */}
      <Show when={showCancelConfirm()}>
        <div
          class="fixed inset-0 z-[100] flex items-center justify-center bg-black/60"
          onClick={(e) => { if (e.target === e.currentTarget) setShowCancelConfirm(false); }}
        >
          <Card class="w-full max-w-md border-red-500/30 p-6">
            <h2 class="mb-3 text-lg font-semibold text-white">Cancel Batch</h2>
            <p class="mb-2 text-sm text-white/70">
              This will cancel all active tasks in batch:
            </p>
            <p class="mb-4 rounded bg-white/5 px-3 py-2 font-mono text-xs text-white/90">
              {batchId()}
            </p>
            <p class="mb-6 text-sm text-red-400/80">
              Running tasks will receive cancel webhooks. This action cannot be undone.
            </p>
            <div class="flex justify-end gap-3">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setShowCancelConfirm(false)}
                disabled={canceling()}
              >
                Keep Running
              </Button>
              <Button
                variant="primary"
                size="sm"
                onClick={handleCancelBatch}
                disabled={canceling()}
                class="!bg-red-600 hover:!bg-red-700"
              >
                {canceling() ? 'Canceling...' : 'Confirm Cancel'}
              </Button>
            </div>
          </Card>
        </div>
      </Show>
    </div>
  );
}

import { createSignal, createEffect, on, onCleanup, onMount, Show, For, Switch, Match } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import type { DagResponse, BasicTask, StopBatchResponse, TaskStatus, UpdateBatchRulesResponse } from '../types';
import { fetchDag, stopBatch, cancelTask } from '../api';
import { addRecentBatch } from '../storage';
import { AUTO_REFRESH_INTERVAL } from '../constants';
import { Button, Card, useWindowManager } from 'glass-ui-solid';
import DagCanvas, { type DagCanvasAPI } from '../components/DagCanvas';
import IsometricView from '../components/IsometricView';
import TaskInfoPanel from '../components/TaskInfoPanel';
import Legend from '../components/Legend';
import StatsBar from '../components/StatsBar';
import StatusFilter from '../components/StatusFilter';
import KindFilter from '../components/KindFilter';
import TaskTable from '../components/TaskTable';
import TimelineView from '../components/TimelineView';
import BatchRulesEditor from '../components/BatchRulesEditor';
import BatchProgress from '../components/BatchProgress';
import KeyboardHelp from '../components/KeyboardHelp';
import { notifyStatusChanges } from '../components/ToastContainer';
import { computeCriticalPath, type CriticalPath } from '../lib/criticalPath';
import { exportTasksCsv } from '../lib/exportCsv';
import { useTheme } from '../App';

export default function DagPage() {
  const params = useParams<{ batchId: string }>();
  const navigate = useNavigate();

  const [dagData, setDagData] = createSignal<DagResponse | null>(null);
  const [openTaskIds, setOpenTaskIds] = createSignal<string[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [viewMode, setViewMode] = createSignal<'dag' | 'iso' | 'table' | 'timeline'>('iso');
  const [activeFilters, setActiveFilters] = createSignal<Set<TaskStatus>>(new Set());
  const [activeKinds, setActiveKinds] = createSignal<Set<string>>(new Set());
  const [isoGroupBy, setIsoGroupBy] = createSignal<'dag' | 'status'>('dag');
  const [message, setMessage] = createSignal<string | null>(null);
  const [showCancelConfirm, setShowCancelConfirm] = createSignal(false);
  const [canceling, setCanceling] = createSignal(false);
  const [showRulesEditor, setShowRulesEditor] = createSignal(false);
  const [showCriticalPath, setShowCriticalPath] = createSignal(false);
  const [showKeyboardHelp, setShowKeyboardHelp] = createSignal(false);

  const { theme, toggle: toggleTheme } = useTheme();

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

  // Combined status + kind filtering
  const filteredData = () => {
    const data = dagData();
    if (!data) return null;
    const statusFilters = activeFilters();
    const kindFilters = activeKinds();
    if (statusFilters.size === 0 && kindFilters.size === 0) return data;
    const tasks = data.tasks.filter((t) => {
      if (statusFilters.size > 0 && !statusFilters.has(t.status)) return false;
      if (kindFilters.size > 0 && !kindFilters.has(t.kind)) return false;
      return true;
    });
    const taskIds = new Set(tasks.map((t) => t.id));
    const links = data.links.filter((l) => taskIds.has(l.parent_id) && taskIds.has(l.child_id));
    return { tasks, links };
  };

  // Critical path computed on full (unfiltered) data
  const criticalPath = (): CriticalPath | null => {
    if (!showCriticalPath()) return null;
    const data = dagData();
    if (!data || data.tasks.length === 0) return null;
    return computeCriticalPath(data.tasks, data.links);
  };

  function toggleStatusFilter(status: TaskStatus) {
    setActiveFilters((prev) => {
      const next = new Set(prev);
      if (next.has(status)) next.delete(status);
      else next.add(status);
      return next;
    });
  }

  function toggleKindFilter(kind: string) {
    setActiveKinds((prev) => {
      const next = new Set(prev);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      return next;
    });
  }

  let canvasApi: DagCanvasAPI | undefined;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  let currentBatchId = '';

  function scheduleRefresh() {
    if (refreshTimer) clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => loadDag(), AUTO_REFRESH_INTERVAL);
  }

  async function loadDag(bid?: string) {
    const batchId = bid ?? params.batchId;
    if (!batchId) return;

    const isNewBatch = batchId !== currentBatchId;
    currentBatchId = batchId;
    addRecentBatch(batchId);

    if (isNewBatch) {
      clearAllWindows();
      setActiveFilters(new Set());
      setActiveKinds(new Set());
    }

    setLoading(true);
    setError(null);
    setMessage(null);

    try {
      const data = await fetchDag(batchId);
      if (data.tasks.length === 0) {
        setMessage('No tasks found for this batch');
        setDagData(null);
        clearAllWindows();
      } else {
        // Detect status changes for toast notifications
        const oldData = dagData();
        if (oldData && !isNewBatch) {
          const oldStatusMap = new Map(
            oldData.tasks.map((t) => [t.id, { name: t.name, status: t.status }]),
          );
          const changes: { taskName: string; fromStatus: TaskStatus; toStatus: TaskStatus }[] = [];
          for (const task of data.tasks) {
            const old = oldStatusMap.get(task.id);
            if (old && old.status !== task.status) {
              changes.push({
                taskName: task.name,
                fromStatus: old.status,
                toStatus: task.status,
              });
            }
          }
          notifyStatusChanges(changes);
        }
        setDagData(data);
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to load DAG';
      setError(msg);
      setMessage(`Error: ${msg}`);
    } finally {
      setLoading(false);
    }

    scheduleRefresh();
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

  function hasActiveTasks(): boolean {
    const data = dagData();
    if (!data) return false;
    return data.tasks.some((t) => !['Success', 'Failure', 'Canceled'].includes(t.status));
  }

  async function handleCancelBatch() {
    const bid = params.batchId;
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
      await loadDag();
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
    setMessage(
      `Rules updated for kind "${result.kind}": ${result.updated_count} task${result.updated_count !== 1 ? 's' : ''} affected`,
    );
    await loadDag();
  }

  let resetIsoCamera: (() => void) | undefined;

  async function handleBulkCancel(taskIds: string[]) {
    if (taskIds.length === 0) return;
    const results = await Promise.allSettled(taskIds.map((id) => cancelTask(id)));
    const failed = results.filter((r) => r.status === 'rejected').length;
    const succeeded = results.filter((r) => r.status === 'fulfilled').length;
    if (failed > 0) {
      setMessage(`Canceled ${succeeded} task${succeeded !== 1 ? 's' : ''}, ${failed} failed`);
    } else {
      setMessage(`Canceled ${succeeded} task${succeeded !== 1 ? 's' : ''}`);
    }
    await loadDag();
  }

  // Keyboard shortcuts
  function handleKeyDown(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;

    switch (e.key) {
      case 'Escape':
        if (showKeyboardHelp()) {
          setShowKeyboardHelp(false);
        } else if (showCancelConfirm()) {
          setShowCancelConfirm(false);
        } else if (showRulesEditor()) {
          setShowRulesEditor(false);
        } else if (openTaskIds().length > 0) {
          clearAllWindows();
        }
        break;
      case '1':
        if (dagData()) setViewMode('dag');
        break;
      case '2':
        if (dagData()) setViewMode('iso');
        break;
      case '3':
        if (dagData()) setViewMode('table');
        break;
      case '4':
        if (dagData()) setViewMode('timeline');
        break;
      case 'f':
      case 'F':
        if (viewMode() === 'dag') canvasApi?.fit();
        if (viewMode() === 'iso') resetIsoCamera?.();
        break;
      case 'c':
      case 'C':
        if (dagData()) setShowCriticalPath((v) => !v);
        break;
      case '?':
        setShowKeyboardHelp((v) => !v);
        break;
    }
  }

  // Load DAG when route param changes
  createEffect(
    on(
      () => params.batchId,
      (bid) => {
        if (bid) loadDag(bid);
      },
    ),
  );

  onMount(() => {
    window.addEventListener('keydown', handleKeyDown);
  });

  onCleanup(() => {
    if (refreshTimer) clearTimeout(refreshTimer);
    window.removeEventListener('keydown', handleKeyDown);
  });

  const batchIdShort = () => {
    const bid = params.batchId;
    return bid ? (bid.length > 12 ? bid.substring(0, 12) + '...' : bid) : '';
  };

  return (
    <div class="flex flex-1 flex-col overflow-hidden">
      {/* Toolbar */}
      <header class="glass-navbar relative z-10 flex flex-wrap items-center gap-3 px-5 py-3">
        {/* Breadcrumb */}
        <div class="flex items-center gap-2 text-sm">
          <button
            class="text-white/50 transition-colors hover:text-white"
            onClick={() => navigate('/')}
          >
            Batches
          </button>
          <span class="text-white/30">/</span>
          <span class="font-mono text-rose-400" title={params.batchId}>
            {batchIdShort()}
          </span>
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <div class="flex overflow-hidden rounded-md border border-white/20">
            <button
              title="2D DAG view (1)"
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
              title="3D isometric view (2)"
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
              title="Table view (3)"
              class="border-r border-white/20 px-2.5 py-1 text-xs font-medium transition-colors"
              classList={{
                'bg-white/15 text-white': viewMode() === 'table',
                'text-white/50 hover:bg-white/5 hover:text-white/80': viewMode() !== 'table',
              }}
              disabled={!dagData()}
              onClick={() => setViewMode('table')}
            >
              Table
            </button>
            <button
              title="Timeline / Gantt view (4)"
              class="px-2.5 py-1 text-xs font-medium transition-colors"
              classList={{
                'bg-white/15 text-white': viewMode() === 'timeline',
                'text-white/50 hover:bg-white/5 hover:text-white/80': viewMode() !== 'timeline',
              }}
              disabled={!dagData()}
              onClick={() => setViewMode('timeline')}
            >
              Timeline
            </button>
          </div>
          <Show when={viewMode() === 'dag' || viewMode() === 'iso'}>
            <Button
              variant="secondary"
              size="sm"
              title="Fit to viewport (F)"
              onClick={() => {
                if (viewMode() === 'dag') canvasApi?.fit();
                else resetIsoCamera?.();
              }}
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
                    'text-white/50 hover:bg-white/5 hover:text-white/80':
                      isoGroupBy() !== 'status',
                  }}
                  onClick={() => setIsoGroupBy('status')}
                >
                  Status
                </button>
              </div>
            </div>
          </Show>

          {/* Critical path toggle */}
          <Show when={dagData()}>
            <button
              title="Toggle critical path (C)"
              class="rounded-md border px-2.5 py-1 text-xs font-medium transition-colors"
              classList={{
                'border-amber-400/40 bg-amber-400/15 text-amber-300': showCriticalPath(),
                'border-white/20 text-white/50 hover:bg-white/5 hover:text-white/80':
                  !showCriticalPath(),
              }}
              onClick={() => setShowCriticalPath((v) => !v)}
            >
              Critical Path
            </button>
          </Show>

          <Show when={openTaskIds().length > 0}>
            <Button variant="secondary" size="sm" onClick={clearAllWindows}>
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

          {/* Export CSV */}
          <Show when={dagData()}>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                const data = filteredData();
                if (data) exportTasksCsv(data.tasks);
              }}
            >
              Export CSV
            </Button>
          </Show>

          {/* Theme toggle */}
          <button
            title={`Switch to ${theme() === 'dark' ? 'light' : 'dark'} mode`}
            class="theme-btn rounded-md border px-2 py-1 text-xs"
            onClick={toggleTheme}
          >
            {theme() === 'dark' ? 'Light' : 'Dark'}
          </button>

          {/* Help */}
          <button
            title="Keyboard shortcuts (?)"
            class="theme-btn rounded-md border px-2 py-1 text-xs"
            onClick={() => setShowKeyboardHelp(true)}
          >
            ?
          </button>

          <span class="hide-mobile">
            <StatsBar tasks={dagData()?.tasks ?? []} linkCount={dagData()?.links.length ?? 0} />
          </span>
          <BatchProgress tasks={dagData()?.tasks ?? []} />
        </div>

        {/* Filters row */}
        <Show when={dagData()}>
          <div class="flex w-full flex-wrap items-center gap-3">
            <StatusFilter
              tasks={dagData()!.tasks}
              activeFilters={activeFilters()}
              onToggle={toggleStatusFilter}
            />
            <Show when={new Set(dagData()!.tasks.map((t) => t.kind)).size > 1}>
              <div class="h-4 w-px bg-white/15" />
              <KindFilter
                tasks={dagData()!.tasks}
                activeKinds={activeKinds()}
                onToggle={toggleKindFilter}
              />
            </Show>
          </div>
        </Show>
      </header>

      {/* Main content */}
      <div class="relative z-1 flex flex-1 overflow-hidden">
        <Show when={loading() && !dagData()}>
          <div class="absolute inset-0 flex items-center justify-center text-white/40">
            Loading...
          </div>
        </Show>
        <Show when={message() && !dagData()}>
          <div class="absolute inset-0 flex items-center justify-center text-white/40">
            {message()}
          </div>
        </Show>

        <Switch>
          <Match when={viewMode() === 'dag'}>
            <DagCanvas
              data={filteredData()}
              criticalPath={criticalPath()}
              onNodeClick={(task) => openTask(task)}
              onBackgroundClick={() => {}}
              ref={(api) => (canvasApi = api)}
            />
          </Match>
          <Match when={viewMode() === 'iso'}>
            <IsometricView
              data={filteredData()}
              criticalPath={criticalPath()}
              onNodeClick={openTask}
              onBackgroundClick={() => {}}
              groupBy={isoGroupBy()}
              onResetCamera={(fn) => (resetIsoCamera = fn)}
            />
          </Match>
          <Match when={viewMode() === 'table'}>
            <TaskTable data={filteredData()} onTaskClick={openTask} onBulkCancel={handleBulkCancel} />
          </Match>
          <Match when={viewMode() === 'timeline'}>
            <TimelineView data={filteredData()} onTaskClick={openTask} />
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

      <BatchRulesEditor
        open={showRulesEditor()}
        batchId={params.batchId}
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

      <KeyboardHelp open={showKeyboardHelp()} onClose={() => setShowKeyboardHelp(false)} />

      {/* Cancel batch confirmation modal */}
      <Show when={showCancelConfirm()}>
        <div
          class="fixed inset-0 z-[100] flex items-center justify-center bg-black/60"
          onClick={(e) => {
            if (e.target === e.currentTarget) setShowCancelConfirm(false);
          }}
        >
          <Card class="w-full max-w-md border-red-500/30 p-6">
            <h2 class="mb-3 text-lg font-semibold text-white">Cancel Batch</h2>
            <p class="mb-2 text-sm text-white/70">
              This will cancel all active tasks in batch:
            </p>
            <p class="mb-4 rounded bg-white/5 px-3 py-2 font-mono text-xs text-white/90">
              {params.batchId}
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

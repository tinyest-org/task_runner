import { createSignal, createEffect, on, onCleanup, onMount, Show, For, Switch, Match } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import type { DagResponse, BasicTask, StopBatchResponse, TaskStatus, UpdateBatchRulesResponse } from '../types';
import { fetchDag, stopBatch, cancelTask } from '../api';
import { addRecentBatch } from '../storage';
import { AUTO_REFRESH_INTERVAL } from '../constants';
import { Card, useWindowManager } from 'glass-ui-solid';
import DagCanvas, { type DagCanvasAPI } from '../components/DagCanvas';
import IsometricView from '../components/IsometricView';
import TaskInfoPanel from '../components/TaskInfoPanel';
import Legend from '../components/Legend';
import TaskTable from '../components/TaskTable';
import TimelineView from '../components/TimelineView';
import BatchRulesEditor from '../components/BatchRulesEditor';
import KeyboardHelp from '../components/KeyboardHelp';
import DagToolbar from '../components/DagToolbar';
import ConfirmModal from '../components/ConfirmModal';
import { notifyStatusChanges } from '../components/ToastContainer';
import { computeCriticalPath, type CriticalPath } from '../lib/criticalPath';
import { exportTasksCsv } from '../lib/exportCsv';
import { useTheme } from '../App';
import ErrorBoundary from '../components/ErrorBoundary';

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
      setActiveFilters(new Set<TaskStatus>());
      setActiveKinds(new Set<string>());
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
      <DagToolbar
        batchId={params.batchId}
        batchIdShort={batchIdShort()}
        dagData={dagData()}
        viewMode={viewMode()}
        isoGroupBy={isoGroupBy()}
        showCriticalPath={showCriticalPath()}
        openTaskCount={openTaskIds().length}
        hasActiveTasks={hasActiveTasks()}
        theme={theme()}
        activeFilters={activeFilters()}
        activeKinds={activeKinds()}
        onNavigateHome={() => navigate('/')}
        onSetViewMode={setViewMode}
        onSetIsoGroupBy={setIsoGroupBy}
        onToggleCriticalPath={() => setShowCriticalPath((v) => !v)}
        onCloseAllWindows={clearAllWindows}
        onEditRules={() => setShowRulesEditor(true)}
        onCancelBatch={() => setShowCancelConfirm(true)}
        onExportCsv={() => {
          const data = filteredData();
          if (data) exportTasksCsv(data.tasks);
        }}
        onToggleTheme={toggleTheme}
        onShowHelp={() => setShowKeyboardHelp(true)}
        onFitView={() => {
          if (viewMode() === 'dag') canvasApi?.fit();
          else resetIsoCamera?.();
        }}
        onToggleStatusFilter={toggleStatusFilter}
        onToggleKindFilter={toggleKindFilter}
      />

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

        <ErrorBoundary>
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
        </ErrorBoundary>
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

      <ConfirmModal
        open={showCancelConfirm()}
        title="Cancel Batch"
        confirmLabel={canceling() ? 'Canceling...' : 'Confirm Cancel'}
        cancelLabel="Keep Running"
        onConfirm={handleCancelBatch}
        onCancel={() => setShowCancelConfirm(false)}
        loading={canceling()}
      >
        <p class="mb-2 text-sm text-white/70">
          This will cancel all active tasks in batch:
        </p>
        <p class="mb-4 rounded bg-white/5 px-3 py-2 font-mono text-xs text-white/90">
          {params.batchId}
        </p>
        <p class="mb-6 text-sm text-red-400/80">
          Running tasks will receive cancel webhooks. This action cannot be undone.
        </p>
      </ConfirmModal>
    </div>
  );
}

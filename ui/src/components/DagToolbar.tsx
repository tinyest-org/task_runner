import { Show } from 'solid-js';
import type { DagResponse, BasicTask, TaskStatus } from '../types';
import { Button } from 'glass-ui-solid';
import StatsBar from './StatsBar';
import StatusFilter from './StatusFilter';
import KindFilter from './KindFilter';
import BatchProgress from './BatchProgress';
import type { Theme } from '../storage';

type ViewMode = 'dag' | 'iso' | 'table' | 'timeline';

interface Props {
  batchId: string;
  batchIdShort: string;
  dagData: DagResponse | null;
  viewMode: ViewMode;
  isoGroupBy: 'dag' | 'status';
  showCriticalPath: boolean;
  openTaskCount: number;
  hasActiveTasks: boolean;
  theme: Theme;
  activeFilters: Set<TaskStatus>;
  activeKinds: Set<string>;
  onNavigateHome: () => void;
  onSetViewMode: (mode: ViewMode) => void;
  onSetIsoGroupBy: (g: 'dag' | 'status') => void;
  onToggleCriticalPath: () => void;
  onCloseAllWindows: () => void;
  onEditRules: () => void;
  onCancelBatch: () => void;
  onExportCsv: () => void;
  onToggleTheme: () => void;
  onShowHelp: () => void;
  onFitView: () => void;
  onToggleStatusFilter: (status: TaskStatus) => void;
  onToggleKindFilter: (kind: string) => void;
}

export default function DagToolbar(props: Props) {
  return (
    <header class="glass-navbar relative z-10 flex flex-wrap items-center gap-3 px-5 py-3">
      {/* Breadcrumb */}
      <div class="flex items-center gap-2 text-sm">
        <button
          class="text-white/50 transition-colors hover:text-white"
          onClick={props.onNavigateHome}
        >
          Batches
        </button>
        <span class="text-white/30">/</span>
        <span class="font-mono text-rose-400" title={props.batchId}>
          {props.batchIdShort}
        </span>
      </div>

      <div class="flex flex-wrap items-center gap-2">
        <div class="flex overflow-hidden rounded-md border border-white/20">
          <button
            title="2D DAG view (1)"
            class="px-2.5 py-1 text-xs font-medium transition-colors"
            classList={{
              'bg-white/15 text-white': props.viewMode === 'dag',
              'text-white/50 hover:bg-white/5 hover:text-white/80': props.viewMode !== 'dag',
            }}
            disabled={!props.dagData}
            onClick={() => props.onSetViewMode('dag')}
          >
            2D
          </button>
          <button
            title="3D isometric view (2)"
            class="border-x border-white/20 px-2.5 py-1 text-xs font-medium transition-colors"
            classList={{
              'bg-white/15 text-white': props.viewMode === 'iso',
              'text-white/50 hover:bg-white/5 hover:text-white/80': props.viewMode !== 'iso',
            }}
            disabled={!props.dagData}
            onClick={() => props.onSetViewMode('iso')}
          >
            3D
          </button>
          <button
            title="Table view (3)"
            class="border-r border-white/20 px-2.5 py-1 text-xs font-medium transition-colors"
            classList={{
              'bg-white/15 text-white': props.viewMode === 'table',
              'text-white/50 hover:bg-white/5 hover:text-white/80': props.viewMode !== 'table',
            }}
            disabled={!props.dagData}
            onClick={() => props.onSetViewMode('table')}
          >
            Table
          </button>
          <button
            title="Timeline / Gantt view (4)"
            class="px-2.5 py-1 text-xs font-medium transition-colors"
            classList={{
              'bg-white/15 text-white': props.viewMode === 'timeline',
              'text-white/50 hover:bg-white/5 hover:text-white/80': props.viewMode !== 'timeline',
            }}
            disabled={!props.dagData}
            onClick={() => props.onSetViewMode('timeline')}
          >
            Timeline
          </button>
        </div>
        <Show when={props.viewMode === 'dag' || props.viewMode === 'iso'}>
          <button
            title="Fit to viewport (F)"
            class="rounded-md border border-white/20 px-2.5 py-1 text-xs font-medium text-white/50 transition-colors hover:bg-white/5 hover:text-white/80"
            classList={{ 'opacity-50 cursor-not-allowed': !props.dagData }}
            disabled={!props.dagData}
            onClick={props.onFitView}
          >
            Fit View
          </button>
        </Show>
        <Show when={props.viewMode === 'iso' && props.dagData}>
          <div class="flex items-center gap-1">
            <span class="text-xs text-white/40">Group:</span>
            <div class="flex overflow-hidden rounded-md border border-white/20">
              <button
                class="px-2 py-0.5 text-xs font-medium transition-colors"
                classList={{
                  'bg-white/15 text-white': props.isoGroupBy === 'dag',
                  'text-white/50 hover:bg-white/5 hover:text-white/80': props.isoGroupBy !== 'dag',
                }}
                onClick={() => props.onSetIsoGroupBy('dag')}
              >
                DAG
              </button>
              <button
                class="border-l border-white/20 px-2 py-0.5 text-xs font-medium transition-colors"
                classList={{
                  'bg-white/15 text-white': props.isoGroupBy === 'status',
                  'text-white/50 hover:bg-white/5 hover:text-white/80':
                    props.isoGroupBy !== 'status',
                }}
                onClick={() => props.onSetIsoGroupBy('status')}
              >
                Status
              </button>
            </div>
          </div>
        </Show>

        {/* Critical path toggle */}
        <Show when={props.dagData}>
          <button
            title="Toggle critical path (C)"
            class="rounded-md border px-2.5 py-1 text-xs font-medium transition-colors"
            classList={{
              'border-amber-400/40 bg-amber-400/15 text-amber-300': props.showCriticalPath,
              'border-white/20 text-white/50 hover:bg-white/5 hover:text-white/80':
                !props.showCriticalPath,
            }}
            onClick={props.onToggleCriticalPath}
          >
            Critical Path
          </button>
        </Show>

        <Show when={props.openTaskCount > 0}>
          <Button variant="secondary" size="sm" onClick={props.onCloseAllWindows}>
            Close All ({props.openTaskCount})
          </Button>
        </Show>
        <Show when={props.dagData && props.hasActiveTasks}>
          <Button
            variant="secondary"
            size="sm"
            onClick={props.onEditRules}
            class="border-cyan-500/40! text-cyan-400! hover:bg-cyan-500/20!"
          >
            Edit Rules
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={props.onCancelBatch}
            class="border-red-500/40! text-red-400! hover:bg-red-500/20!"
          >
            Cancel Batch
          </Button>
        </Show>

        {/* Export CSV */}
        <Show when={props.dagData}>
          <Button
            variant="secondary"
            size="sm"
            onClick={props.onExportCsv}
          >
            Export CSV
          </Button>
        </Show>

        {/* Theme toggle */}
        <button
          title={`Switch to ${props.theme === 'dark' ? 'light' : 'dark'} mode`}
          class="theme-btn rounded-md border px-2 py-1 text-xs"
          onClick={props.onToggleTheme}
        >
          {props.theme === 'dark' ? 'Light' : 'Dark'}
        </button>

        {/* Help */}
        <button
          title="Keyboard shortcuts (?)"
          class="theme-btn rounded-md border px-2 py-1 text-xs"
          onClick={props.onShowHelp}
        >
          ?
        </button>

        <span class="hide-mobile">
          <StatsBar tasks={props.dagData?.tasks ?? []} linkCount={props.dagData?.links.length ?? 0} />
        </span>
        <BatchProgress tasks={props.dagData?.tasks ?? []} />
      </div>

      {/* Filters row */}
      <Show when={props.dagData}>
        <div class="flex w-full flex-wrap items-center gap-3">
          <StatusFilter
            tasks={props.dagData!.tasks}
            activeFilters={props.activeFilters}
            onToggle={props.onToggleStatusFilter}
          />
          <Show when={new Set(props.dagData!.tasks.map((t: BasicTask) => t.kind)).size > 1}>
            <div class="h-4 w-px bg-white/15" />
            <KindFilter
              tasks={props.dagData!.tasks}
              activeKinds={props.activeKinds}
              onToggle={props.onToggleKindFilter}
            />
          </Show>
        </div>
      </Show>
    </header>
  );
}

import type { BasicTask, Link, TaskStatus } from '../types';
import { ALL_STATUSES } from '../constants';

export interface GroupLayout {
  key: { kind: string; step: number };
  tasks: BasicTask[];
  kindIndex: number;
  stepIndex: number;
  cols: number;
  rows: number;
  layers: number;
}

export interface LayoutResult {
  groups: GroupLayout[];
  kinds: string[];
  maxStep: number;
  depthMap: Map<string, number>;
  maxCols: number;
  maxRows: number;
  maxLayers: number;
}

/**
 * Compute topological depth for each task using modified Kahn's algorithm.
 * Each task gets step = max(step of parents) + 1. Roots are step 0.
 */
export function computeTopologicalDepth(
  tasks: BasicTask[],
  links: Link[],
): Map<string, number> {
  const taskIds = new Set(tasks.map((t) => t.id));
  const inEdges = new Map<string, { parentId: string }[]>();
  const outEdges = new Map<string, string[]>();
  const depth = new Map<string, number>();

  for (const id of taskIds) {
    inEdges.set(id, []);
    outEdges.set(id, []);
  }

  for (const link of links) {
    if (!taskIds.has(link.parent_id) || !taskIds.has(link.child_id)) continue;
    inEdges.get(link.child_id)!.push({ parentId: link.parent_id });
    outEdges.get(link.parent_id)!.push(link.child_id);
  }

  // BFS from roots
  const inDegree = new Map<string, number>();
  for (const id of taskIds) {
    inDegree.set(id, inEdges.get(id)!.length);
  }

  const queue: string[] = [];
  for (const id of taskIds) {
    if (inDegree.get(id) === 0) {
      depth.set(id, 0);
      queue.push(id);
    }
  }

  let head = 0;
  while (head < queue.length) {
    const current = queue[head++];
    const currentDepth = depth.get(current)!;

    for (const childId of outEdges.get(current)!) {
      const childDepth = depth.get(childId);
      const newDepth = currentDepth + 1;
      if (childDepth === undefined || newDepth > childDepth) {
        depth.set(childId, newDepth);
      }

      const remaining = inDegree.get(childId)! - 1;
      inDegree.set(childId, remaining);
      if (remaining === 0) {
        queue.push(childId);
      }
    }
  }

  // Tasks not reached (cycles or disconnected) get step 0
  for (const id of taskIds) {
    if (!depth.has(id)) {
      depth.set(id, 0);
    }
  }

  return depth;
}

/**
 * Status shell priority for radial placement within a group cube.
 * 0 = outermost (edge of the cube), 2 = innermost (center).
 */
function statusShellPriority(status: TaskStatus): number {
  switch (status) {
    case 'Running':
    case 'Failure':
    case 'Canceled':
      return 0; // outermost
    case 'Pending':
    case 'Claimed':
    case 'Waiting':
    case 'Paused':
      return 1; // middle
    case 'Success':
      return 2; // innermost
    default:
      return 1;
  }
}

/**
 * Reorder tasks so that when placed sequentially in a 3D grid (cols x rows x layers),
 * Running/Failed tasks end up near the edges and Success tasks near the center.
 */
function sortTasksForRadialPlacement(
  tasks: BasicTask[],
  cols: number,
  rows: number,
): BasicTask[] {
  const n = tasks.length;
  if (n <= 1) return tasks;

  const sliceSize = cols * rows;
  const layers = Math.ceil(n / sliceSize);
  const centerCol = (cols - 1) / 2;
  const centerRow = (rows - 1) / 2;
  const centerLayer = (layers - 1) / 2;

  // Compute distance from center for each grid index
  const indexDistances: { index: number; dist: number }[] = [];
  for (let i = 0; i < n; i++) {
    const layer = Math.floor(i / sliceSize);
    const inSlice = i % sliceSize;
    const col = inSlice % cols;
    const row = Math.floor(inSlice / cols);

    const dx = cols > 1 ? (col - centerCol) / centerCol : 0;
    const dz = rows > 1 ? (row - centerRow) / centerRow : 0;
    const dy = layers > 1 ? (layer - centerLayer) / centerLayer : 0;
    const dist = Math.sqrt(dx * dx + dy * dy + dz * dz);
    indexDistances.push({ index: i, dist });
  }

  // Sort slots: outermost first (descending distance)
  indexDistances.sort((a, b) => b.dist - a.dist);

  // Sort tasks: outermost-priority first (Running/Failed -> Pending -> Success)
  const sortedTasks = [...tasks].sort(
    (a, b) => statusShellPriority(a.status) - statusShellPriority(b.status),
  );

  // Assign: first sorted task (Running/Failed) -> outermost slot, etc.
  const result = new Array<BasicTask>(n);
  for (let i = 0; i < n; i++) {
    result[indexDistances[i].index] = sortedTasks[i];
  }

  return result;
}

/**
 * Group tasks by (kind, step) and return layout information.
 */
export function computeGroupLayout(
  tasks: BasicTask[],
  links: Link[],
): LayoutResult {
  const depthMap = computeTopologicalDepth(tasks, links);

  // Collect unique kinds sorted alphabetically
  const kindSet = new Set<string>();
  for (const t of tasks) kindSet.add(t.kind);
  const kinds = Array.from(kindSet).sort();
  const kindIndex = new Map<string, number>();
  kinds.forEach((k, i) => kindIndex.set(k, i));

  // Find max step
  let maxStep = 0;
  for (const d of depthMap.values()) {
    if (d > maxStep) maxStep = d;
  }

  // Group tasks by (kind, step)
  const groupMap = new Map<string, BasicTask[]>();
  for (const task of tasks) {
    const step = depthMap.get(task.id) ?? 0;
    const key = `${task.kind}:${step}`;
    let group = groupMap.get(key);
    if (!group) {
      group = [];
      groupMap.set(key, group);
    }
    group.push(task);
  }

  // Build GroupLayout array
  const groups: GroupLayout[] = [];
  let maxCols = 1;
  let maxRows = 1;
  let maxLayers = 1;
  for (const [key, groupTasks] of groupMap) {
    const [kind, stepStr] = key.split(':');
    const step = parseInt(stepStr, 10);
    const n = groupTasks.length;
    const dim = Math.ceil(Math.cbrt(n));
    const cols = dim;
    const rows = Math.ceil(Math.sqrt(n / cols));
    const layers = Math.ceil(n / (cols * rows));
    if (cols > maxCols) maxCols = cols;
    if (rows > maxRows) maxRows = rows;
    if (layers > maxLayers) maxLayers = layers;
    groups.push({
      key: { kind, step },
      tasks: sortTasksForRadialPlacement(groupTasks, cols, rows),
      kindIndex: kindIndex.get(kind)!,
      stepIndex: step,
      cols,
      rows,
      layers,
    });
  }

  return { groups, kinds, maxStep, depthMap, maxCols, maxRows, maxLayers };
}

// --- Status-based layout (DAG steps × status) ---

export interface StatusGroupLayout {
  status: TaskStatus;
  step: number;
  tasks: BasicTask[];
  statusIndex: number;
  stepIndex: number;
  cols: number;
  rows: number;
  layers: number;
}

export interface StatusLayoutResult {
  groups: StatusGroupLayout[];
  statuses: TaskStatus[];
  maxStep: number;
  maxCols: number;
  maxRows: number;
  maxLayers: number;
}

/**
 * Group tasks by DAG step first, then by status within each step.
 * Preserves topological structure while sub-grouping by status.
 * X axis = status columns (globally aligned), Z axis = DAG step rows.
 */
export function computeStatusLayout(tasks: BasicTask[], links: Link[] = []): StatusLayoutResult {
  const depthMap = computeTopologicalDepth(tasks, links);

  // Find max step
  let maxStep = 0;
  for (const d of depthMap.values()) {
    if (d > maxStep) maxStep = d;
  }

  // Group tasks by (step, status)
  const groupMap = new Map<string, BasicTask[]>();
  for (const task of tasks) {
    const step = depthMap.get(task.id) ?? 0;
    const key = `${step}:${task.status}`;
    let group = groupMap.get(key);
    if (!group) {
      group = [];
      groupMap.set(key, group);
    }
    group.push(task);
  }

  // Collect all present statuses in canonical order
  const presentStatuses = new Set<TaskStatus>();
  for (const task of tasks) presentStatuses.add(task.status);
  const statuses = ALL_STATUSES.filter((s) => presentStatuses.has(s));
  const statusIndex = new Map<TaskStatus, number>();
  statuses.forEach((s, i) => statusIndex.set(s, i));

  const groups: StatusGroupLayout[] = [];
  let maxCols = 1;
  let maxRows = 1;
  let maxLayers = 1;

  for (const [key, groupTasks] of groupMap) {
    const colonIdx = key.indexOf(':');
    const step = parseInt(key.substring(0, colonIdx), 10);
    const status = key.substring(colonIdx + 1) as TaskStatus;
    groupTasks.sort((a, b) => a.id.localeCompare(b.id));
    const n = groupTasks.length;
    const dim = Math.ceil(Math.cbrt(n));
    const cols = dim;
    const rows = Math.ceil(Math.sqrt(n / cols));
    const layers = Math.ceil(n / (cols * rows));
    if (cols > maxCols) maxCols = cols;
    if (rows > maxRows) maxRows = rows;
    if (layers > maxLayers) maxLayers = layers;
    groups.push({
      status,
      step,
      tasks: groupTasks,
      statusIndex: statusIndex.get(status)!,
      stepIndex: step,
      cols,
      rows,
      layers,
    });
  }

  return { groups, statuses, maxStep, maxCols, maxRows, maxLayers };
}

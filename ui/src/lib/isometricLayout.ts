import type { BasicTask, Link } from '../types';

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
      tasks: groupTasks,
      kindIndex: kindIndex.get(kind)!,
      stepIndex: step,
      cols,
      rows,
      layers,
    });
  }

  return { groups, kinds, maxStep, depthMap, maxCols, maxRows, maxLayers };
}

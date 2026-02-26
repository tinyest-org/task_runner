import type { BasicTask, Link } from '../types';
import { computeTopologicalDepth } from './isometricLayout';

export interface CriticalPath {
  nodeIds: Set<string>;
  edgeIds: Set<string>;
}

/**
 * Compute the critical path (longest path) in the DAG.
 * Uses topological depth: backtrack from the deepest node to a root,
 * always choosing the parent with depth = current - 1.
 */
export function computeCriticalPath(tasks: BasicTask[], links: Link[]): CriticalPath {
  if (tasks.length === 0) return { nodeIds: new Set(), edgeIds: new Set() };

  const depthMap = computeTopologicalDepth(tasks, links);

  // Find the node with maximum depth
  let maxDepth = 0;
  let endNodeId = tasks[0].id;
  for (const [id, depth] of depthMap) {
    if (depth > maxDepth) {
      maxDepth = depth;
      endNodeId = id;
    }
  }

  // No meaningful path if everything is at depth 0
  if (maxDepth === 0) return { nodeIds: new Set(), edgeIds: new Set() };

  // Build parent lookup
  const taskIds = new Set(tasks.map((t) => t.id));
  const parents = new Map<string, string[]>();
  for (const link of links) {
    if (!taskIds.has(link.parent_id) || !taskIds.has(link.child_id)) continue;
    const list = parents.get(link.child_id) ?? [];
    list.push(link.parent_id);
    parents.set(link.child_id, list);
  }

  // Backtrack from end node
  const nodeIds = new Set<string>();
  const edgeIds = new Set<string>();

  let current = endNodeId;
  nodeIds.add(current);

  while (depthMap.get(current)! > 0) {
    const parentList = parents.get(current) ?? [];
    const currentDepth = depthMap.get(current)!;
    let bestParent: string | null = null;
    for (const parentId of parentList) {
      if (depthMap.get(parentId) === currentDepth - 1) {
        bestParent = parentId;
        break;
      }
    }
    if (!bestParent) break;
    nodeIds.add(bestParent);
    edgeIds.add(`${bestParent}-${current}`);
    current = bestParent;
  }

  return { nodeIds, edgeIds };
}

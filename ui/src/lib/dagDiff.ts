import type { BasicTask, Link } from '../types';

/** Check whether the DAG structure (nodes/edges) changed vs previous snapshot. */
export function structureChanged(
  tasks: BasicTask[],
  links: Link[],
  currentNodeIds: Set<string>,
  currentEdgeIds: Set<string>,
): boolean {
  if (tasks.length !== currentNodeIds.size) return true;
  for (const t of tasks) {
    if (!currentNodeIds.has(t.id)) return true;
  }
  const newEdgeIds = new Set(links.map((l) => `${l.parent_id}-${l.child_id}`));
  if (newEdgeIds.size !== currentEdgeIds.size) return true;
  for (const eid of newEdgeIds) {
    if (!currentEdgeIds.has(eid)) return true;
  }
  return false;
}

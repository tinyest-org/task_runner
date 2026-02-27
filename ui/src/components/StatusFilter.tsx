import type { BasicTask, TaskStatus } from '../types';
import { ALL_STATUSES, STATUS_COLORS } from '../constants';
import ToggleFilter from './ToggleFilter';

interface Props {
  tasks: BasicTask[];
  activeFilters: Set<TaskStatus>;
  onToggle: (status: TaskStatus) => void;
}

export default function StatusFilter(props: Props) {
  const items = () => {
    const counts: Partial<Record<TaskStatus, number>> = {};
    for (const t of props.tasks) {
      counts[t.status] = (counts[t.status] ?? 0) + 1;
    }
    return ALL_STATUSES
      .filter((s) => (counts[s] ?? 0) > 0)
      .map((s) => ({ key: s, label: s, count: counts[s]!, color: STATUS_COLORS[s] }));
  };

  return (
    <ToggleFilter
      items={items()}
      active={props.activeFilters as Set<string>}
      onToggle={(key) => props.onToggle(key as TaskStatus)}
      activeClass="border-white/40 text-white"
    />
  );
}

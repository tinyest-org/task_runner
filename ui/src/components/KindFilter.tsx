import type { BasicTask } from '../types';
import ToggleFilter from './ToggleFilter';

interface Props {
  tasks: BasicTask[];
  activeKinds: Set<string>;
  onToggle: (kind: string) => void;
}

export default function KindFilter(props: Props) {
  const items = () => {
    const map = new Map<string, number>();
    for (const t of props.tasks) {
      map.set(t.kind, (map.get(t.kind) ?? 0) + 1);
    }
    return Array.from(map.entries())
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([kind, count]) => ({ key: kind, label: kind, count }));
  };

  return (
    <ToggleFilter
      items={items()}
      active={props.activeKinds}
      onToggle={props.onToggle}
    />
  );
}

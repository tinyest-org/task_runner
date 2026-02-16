const RECENT_BATCHES_KEY = 'task-runner-recent-batches';
const MAX_RECENT = 10;

export function getRecentBatches(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_BATCHES_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

export function addRecentBatch(id: string): void {
  const recents = getRecentBatches().filter((b) => b !== id);
  recents.unshift(id);
  if (recents.length > MAX_RECENT) recents.length = MAX_RECENT;
  localStorage.setItem(RECENT_BATCHES_KEY, JSON.stringify(recents));
}

import { ErrorBoundary as SolidErrorBoundary } from 'solid-js';
import type { JSX } from 'solid-js';
import { Button } from 'glass-ui-solid';

interface Props {
  children: JSX.Element;
  fallback?: (err: Error, reset: () => void) => JSX.Element;
}

function DefaultFallback(props: { error: Error; reset: () => void }) {
  return (
    <div class="flex flex-col items-center justify-center gap-3 p-8 text-center">
      <p class="text-sm text-white/60">Something went wrong</p>
      <p class="max-w-md text-xs text-white/40">{props.error.message}</p>
      <Button variant="secondary" size="sm" onClick={props.reset}>
        Try again
      </Button>
    </div>
  );
}

export default function ErrorBoundary(props: Props) {
  return (
    <SolidErrorBoundary
      fallback={(err, reset) =>
        props.fallback
          ? props.fallback(err instanceof Error ? err : new Error(String(err)), reset)
          : DefaultFallback({ error: err instanceof Error ? err : new Error(String(err)), reset })
      }
    >
      {props.children}
    </SolidErrorBoundary>
  );
}

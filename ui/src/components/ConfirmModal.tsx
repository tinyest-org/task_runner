import { Show } from 'solid-js';
import type { JSX } from 'solid-js';
import { Card, Button } from 'glass-ui-solid';

interface Props {
  open: boolean;
  title: string;
  children: JSX.Element;
  confirmLabel?: string;
  cancelLabel?: string;
  confirmClass?: string;
  onConfirm: () => void;
  onCancel: () => void;
  loading?: boolean;
}

export default function ConfirmModal(props: Props) {
  return (
    <Show when={props.open}>
      <div
        class="fixed inset-0 z-100 flex items-center justify-center bg-black/60"
        onClick={(e) => {
          if (e.target === e.currentTarget) props.onCancel();
        }}
      >
        <Card class="w-full max-w-md border-red-500/30 p-6">
          <h2 class="mb-3 text-lg font-semibold text-white">{props.title}</h2>
          {props.children}
          <div class="flex justify-end gap-3">
            <Button
              variant="secondary"
              size="sm"
              onClick={props.onCancel}
              disabled={props.loading}
            >
              {props.cancelLabel ?? 'Cancel'}
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={props.onConfirm}
              disabled={props.loading}
              class={props.confirmClass ?? 'bg-red-600! hover:bg-red-700!'}
            >
              {props.loading ? 'Processing...' : (props.confirmLabel ?? 'Confirm')}
            </Button>
          </div>
        </Card>
      </div>
    </Show>
  );
}

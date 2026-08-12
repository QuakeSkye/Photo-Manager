import { useEffect, useRef, type KeyboardEvent, type ReactNode } from 'react';

interface ModalProps {
  readonly title: string;
  readonly children: ReactNode;
  readonly confirmLabel: string;
  readonly className?: string;
  readonly busy?: boolean;
  readonly confirmDisabled?: boolean;
  readonly cancelLabel?: string;
  readonly cancelDisabled?: boolean;
  readonly dismissDisabled?: boolean;
  readonly confirmBusyLabel?: string;
  readonly onConfirm: () => void;
  readonly onCancel: () => void;
}

export function Modal({
  title,
  children,
  confirmLabel,
  className,
  busy = false,
  confirmDisabled = false,
  cancelLabel = '取消',
  cancelDisabled = false,
  dismissDisabled = false,
  confirmBusyLabel = '正在确认…',
  onConfirm,
  onCancel,
}: ModalProps) {
  const dialogRef = useRef<HTMLElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    confirmRef.current?.focus();
  }, []);
  useEffect(() => {
    if (
      (busy || confirmDisabled) &&
      document.activeElement === confirmRef.current &&
      !busy &&
      !cancelDisabled
    ) {
      cancelRef.current?.focus();
    }
  }, [busy, cancelDisabled, confirmDisabled]);

  const handleKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && !busy && !dismissDisabled) {
      event.preventDefault();
      onCancel();
      return;
    }
    if (event.key !== 'Tab') return;
    const controls = Array.from(
      dialogRef.current?.querySelectorAll<HTMLButtonElement>(
        'button:not(:disabled)',
      ) ?? [],
    );
    if (controls.length === 0) return;
    const first = controls[0];
    const last = controls.at(-1);
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        ref={dialogRef}
        className={className === undefined ? 'modal' : `modal ${className}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
        onKeyDown={handleKeyDown}
      >
        <h2 id="modal-title">{title}</h2>
        <div className="modal-body">{children}</div>
        <div className="modal-actions">
          <button
            ref={cancelRef}
            type="button"
            onClick={onCancel}
            disabled={busy || cancelDisabled}
          >
            {cancelLabel}
          </button>
          <button
            ref={confirmRef}
            className="button-primary"
            type="button"
            onClick={onConfirm}
            disabled={busy || confirmDisabled}
          >
            {busy ? confirmBusyLabel : confirmLabel}
          </button>
        </div>
      </section>
    </div>
  );
}

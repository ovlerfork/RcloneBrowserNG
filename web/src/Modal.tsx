import { useEffect, useRef, type ReactNode } from "react";

export function Modal({
  children,
  onClose,
  title,
  confirmation = false,
}: {
  children: ReactNode;
  onClose: () => void;
  title: string;
  confirmation?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  return (
    <dialog
      ref={ref}
      className={`overlay ${confirmation ? "confirmation" : ""}`}
      aria-labelledby={title}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      {children}
    </dialog>
  );
}

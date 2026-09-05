import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { postNative } from "../shared/native";
import type { AppModalState } from "./types";
import { CreateTaskForm } from "./CreateTaskForm";
import { CreateProjectForm } from "./CreateProjectForm";

/** One themed, focus-contained surface for confirmations and project forms. */
export function AppModal({ modal, language, onDismiss }: {
  modal: AppModalState; language: "en" | "es"; onDismiss: () => void;
}) {
  const section = useRef<HTMLElement>(null);
  const [pending, setPending] = useState(false);
  const [offset, setOffset] = useState(0);
  const isProject = modal.kind === "create_project";
  const isTask = modal.kind === "create_task";
  const isForm = isProject || isTask;

  useEffect(() => {
    if (pending) section.current?.focus();
  }, [pending]);

  useLayoutEffect(() => {
    const element = section.current;
    if (!element) return;
    const resize = () => {
      const available = Math.max(0, (window.innerWidth - element.offsetWidth) / 2 - 28);
      setOffset(Math.max(-available, Math.min(available, modal.offset_x || 0)));
    };
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    window.addEventListener("resize", resize);
    resize();
    const previous = document.activeElement as HTMLElement | null;
    const siblings = [...(element.parentElement?.parentElement?.children || [])]
      .filter((child): child is HTMLElement => child instanceof HTMLElement && child !== element.parentElement);
    const inert = siblings.map(child => child.inert);
    siblings.forEach(child => { child.inert = true; });
    element.querySelector<HTMLElement>("[data-initial-focus]")?.focus();
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", resize);
      siblings.forEach((child, index) => { child.inert = inert[index]; });
      if (previous?.isConnected) previous.focus();
    };
  }, [modal.offset_x]);

  const dismiss = () => { if (!pending) onDismiss(); };

  return <div className="app-modal-backdrop" onMouseDown={event => {
    if (event.target === event.currentTarget) dismiss();
  }}>
    <section ref={section} className={`app-modal${isForm ? " app-modal--form" : ""}${isTask ? " app-modal--task" : ""}${isProject ? " app-modal--project" : ""}`}
      role={isForm ? "dialog" : "alertdialog"} aria-modal="true" aria-busy={pending} tabIndex={-1}
      aria-labelledby="app-modal-title" aria-describedby="app-modal-description"
      style={{ transform: `translateX(${offset}px)` }}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); dismiss(); }
        if (event.key === "Tab") {
          const items = [...(section.current?.querySelectorAll<HTMLElement>(
            'button:enabled, input:enabled, textarea:enabled, select:enabled, [tabindex="0"]') || [])].filter(item => item.getClientRects().length > 0);
          const first = items[0], last = items[items.length - 1];
          if (!first) { event.preventDefault(); }
          else if (document.activeElement === section.current) { event.preventDefault(); (event.shiftKey ? last : first)?.focus(); }
          else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
      <h2 id="app-modal-title">{isTask ? (language === "es" ? "Crear tarea" : "Create task") : modal.title}</h2>
      {isTask ? <CreateTaskForm modal={modal} language={language} onDismiss={dismiss} onBusyChange={setPending} /> : isProject ? <CreateProjectForm modal={modal} language={language} onDismiss={dismiss} onBusyChange={setPending} /> : <>
        <strong>{modal.name}</strong>
        {modal.context && <span className="app-modal__context">{modal.context}</span>}
        <p id="app-modal-description">{modal.description}</p>
        <footer>
          <button type="button" className="app-modal__cancel" data-initial-focus onClick={dismiss}>{modal.cancel_label}</button>
          <button type="button" className="app-modal__confirm" onClick={() => {
            postNative(modal.kind === "remove_agent"
              ? { type: "confirm_remove_agent", scope: modal.scope }
              : modal.kind === "remove_task" ? { type: "confirm_remove_task", task_id: modal.task_id }
              : { type: "confirm_remove_project", workspace_id: modal.workspace_id });
          }}>{modal.confirm_label}</button>
        </footer>
      </>}
    </section>
  </div>;
}

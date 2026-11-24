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
  const isProject = modal.kind === "create_project" || modal.kind === "add_repository";
  const isTask = modal.kind === "create_task";
  const isRename = modal.kind === "rename_terminal" || modal.kind === "agent_section";
  const isForm = isProject || isTask || isRename;
  useEffect(() => { if (modal.feedback) setPending(false); }, [modal.feedback]);

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
    <section ref={section} className={`app-modal${isForm && !isRename ? " app-modal--form" : ""}${isTask ? " app-modal--task" : ""}${isProject ? " app-modal--project" : ""}`}
      role={isForm ? "dialog" : "alertdialog"} aria-modal="true" aria-busy={pending} tabIndex={-1}
      aria-labelledby="app-modal-title" aria-describedby="app-modal-description"
      style={{ transform: `translateX(${offset}px)` }}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); dismiss(); }
        if (event.key === "Tab") {
          const items = [...(section.current?.querySelectorAll<HTMLElement>(
            'button:enabled, input:enabled, textarea:enabled, select:enabled, [tabindex="0"]') || [])].filter(item => item.tabIndex >= 0 && item.getClientRects().length > 0);
          const first = items[0], last = items[items.length - 1];
          if (!first) { event.preventDefault(); }
          else if (document.activeElement === section.current) { event.preventDefault(); (event.shiftKey ? last : first)?.focus(); }
          else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
      <h2 id="app-modal-title">{isTask ? (language === "es" ? "Crear tarea" : "Create task") : modal.title}</h2>
      {isTask ? <CreateTaskForm modal={modal} language={language} onDismiss={dismiss} onBusyChange={setPending} /> : isProject ? <CreateProjectForm modal={modal} language={language} onDismiss={dismiss} onBusyChange={setPending} /> : isRename ? <NameForm modal={modal} pending={pending} onDismiss={dismiss} onSubmit={(name, note) => {
        setPending(true);
        postNative({ type: "submit_name_modal", request_id: modal.request_id, name, note });
      }} /> : <>
        <strong>{modal.name}</strong>
        {modal.context && <span className="app-modal__context">{modal.context}</span>}
        <p id="app-modal-description">{modal.description}</p>
        {modal.close_preview && <div style={{ maxHeight: "35vh", overflow: "auto", overflowWrap: "anywhere" }}>
          <b>{language === "es" ? "Terminales que se cerrarán" : "Terminals to close"}</b>
          <ul>{modal.close_preview.terminals.map(terminal => <li key={terminal.terminalId}>
            {terminal.label} · {terminal.state} · {terminal.live
              ? (language === "es" ? "proceso vivo" : "live process")
              : (language === "es" ? "sin proceso" : "no process")}
          </li>)}</ul>
          <b>{language === "es" ? "Cambios y archivos ignorados que se borrarán" : "Changes and ignored files to discard"}</b>
          {modal.close_preview.changes.map(change => <div key={change.path}>
            <code>{change.path}</code><pre style={{ whiteSpace: "pre-wrap" }}>{change.changes}</pre>
          </div>)}
          <b>{language === "es" ? "Archivos extra que se borrarán" : "Extra files to discard"}</b>
          <ul>{modal.close_preview.extraFiles.map(path => <li key={path}><code>{path}</code></li>)}</ul>
        </div>}
        {modal.feedback?.error && <p className="app-modal__error" role="alert">{modal.feedback.error}</p>}
        <footer>
          <button type="button" className="app-modal__cancel" disabled={pending} data-initial-focus onClick={dismiss}>{modal.cancel_label}</button>
          <button type="button" className="app-modal__confirm" disabled={pending} onClick={() => {
            if (modal.kind === "remove_repository") {
              setPending(true);
              postNative({ type: "confirm_remove_repository", request_id: modal.request_id });
              return;
            }
            postNative(modal.kind === "close_terminal" ? { type: "confirm_close_terminal", terminal_id: modal.terminal_id }
              : modal.kind === "remove_task" ? { type: "confirm_remove_task", task_id: modal.task_id }
              : { type: "confirm_remove_project", workspace_id: modal.workspace_id });
          }}>{modal.confirm_label}</button>
        </footer>
      </>}
    </section>
  </div>;
}

/** Names a session (with an optional note) or a section; the name starts selected so typing replaces it. */
function NameForm({ modal, pending, onDismiss, onSubmit }: {
  modal: AppModalState; pending: boolean; onDismiss: () => void; onSubmit: (name: string, note: string | null) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(modal.value || "");
  const [note, setNote] = useState(modal.note ?? "");
  const hasNote = modal.note !== undefined && modal.note !== null;
  // A section needs a name; a session's empty name returns it to the agent's title.
  const blocked = modal.kind === "agent_section" && !name.trim();
  useEffect(() => { input.current?.focus(); input.current?.select(); }, []);
  return <>
    {modal.name && <strong>{modal.name}</strong>}
    {modal.context && <span className="app-modal__context">{modal.context}</span>}
    <form className="app-modal__name-form" onSubmit={event => {
      event.preventDefault();
      if (!pending && !blocked) onSubmit(name, hasNote ? note : null);
    }}>
      <input ref={input} type="text" value={name} maxLength={modal.value_max || 72} disabled={pending} data-initial-focus
        aria-label={modal.title} aria-describedby="app-modal-description" spellCheck={false} autoComplete="off"
        onChange={event => setName(event.target.value)} />
      {hasNote && <label className="app-modal__note">{modal.note_label}
        <textarea value={note} rows={2} maxLength={modal.note_max || 280} disabled={pending} placeholder={modal.note_placeholder}
          onChange={event => setNote(event.target.value)}
          onKeyDown={event => {
            // Enter saves, as in the name field; Shift+Enter is not needed for one line.
            if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit(); }
          }} />
      </label>}
      <p id="app-modal-description" className="app-modal__hint">{modal.description}</p>
      <footer>
        <button type="button" className="app-modal__cancel" disabled={pending} onClick={onDismiss}>{modal.cancel_label}</button>
        <button type="submit" className="app-modal__primary" disabled={pending || blocked}>{modal.confirm_label}</button>
      </footer>
    </form>
  </>;
}

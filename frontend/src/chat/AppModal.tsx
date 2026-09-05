import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { FolderOpen, GitBranch, Plus } from "lucide-react";
import { postNative } from "../shared/native";
import type { AppModalState } from "./types";

/** One themed, focus-contained surface for confirmations and project forms. */
export function AppModal({ modal, language, onDismiss }: {
  modal: AppModalState; language: "en" | "es"; onDismiss: () => void;
}) {
  const section = useRef<HTMLElement>(null);
  const [mode, setMode] = useState<"empty" | "existing" | "github">("empty");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [path, setPath] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [offset, setOffset] = useState(0);
  const isProject = modal.kind === "create_project";
  const tr = (en: string, es: string) => language === "en" ? en : es;

  useEffect(() => {
    if (!modal.feedback) return;
    setPending(false);
    setError(modal.feedback.error || "");
    if (modal.feedback.path != null) setPath(modal.feedback.path);
  }, [modal.feedback]);

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
  const valid = mode === "empty" ? Boolean(name.trim()) : mode === "existing" ? Boolean(path) : Boolean(url.trim());

  return <div className="app-modal-backdrop" onMouseDown={event => {
    if (event.target === event.currentTarget) dismiss();
  }}>
    <section ref={section} className={`app-modal${isProject ? " app-modal--form" : ""}`}
      role={isProject ? "dialog" : "alertdialog"} aria-modal="true" aria-busy={pending} tabIndex={-1}
      aria-labelledby="app-modal-title" aria-describedby="app-modal-description"
      style={{ transform: `translateX(${offset}px)` }}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); dismiss(); }
        if (event.key === "Tab") {
          const items = [...(section.current?.querySelectorAll<HTMLElement>(
            'button:not(:disabled), input:not(:disabled), [tabindex="0"]') || [])];
          const first = items[0], last = items[items.length - 1];
          if (!first) { event.preventDefault(); }
          else if (document.activeElement === section.current) { event.preventDefault(); (event.shiftKey ? last : first)?.focus(); }
          else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
      <h2 id="app-modal-title">{modal.title}</h2>
      {isProject ? <form onSubmit={event => {
        event.preventDefault();
        if (!valid || pending) return;
        setError(""); setPending(true);
        postNative({ type: "submit_create_project", request_id: modal.request_id, mode, name, url, path });
      }}>
        <p id="app-modal-description">{tr("Each project has its own folder, repositories, skills, and instructions. Local imports clone committed files; your originals stay untouched.", "Cada proyecto tiene su carpeta, repositorios, skills e instrucciones. Las importaciones locales clonan archivos con commit; los originales no se modifican.")}</p>
        <fieldset disabled={pending} className="project-modal-fields">
          <legend className="sr-only">{tr("Project source", "Origen del proyecto")}</legend>
          <div className="project-modal-modes">
            {([
              ["empty", Plus, tr("Empty", "Vacío")],
              ["existing", FolderOpen, tr("Clone local", "Clonar local")],
              ["github", GitBranch, "GitHub"],
            ] as const).map(([value, Icon, label]) => <button key={value} type="button"
              aria-pressed={mode === value} onClick={() => { setMode(value); setError(""); }}>
              <Icon size={16} />{label}
            </button>)}
          </div>
          {mode === "github" && <label>{tr("Repository URL", "URL del repositorio")}
            <input type="text" required value={url} placeholder="https://github.com/owner/repository"
              onChange={event => setUrl(event.target.value)} spellCheck={false} />
          </label>}
          {mode === "existing" && <div className="project-modal-folder">
            <span>{tr("Project folder", "Carpeta del proyecto")}</span>
            <button type="button" onClick={() => {
              setPending(true);
              postNative({ type: "choose_project_modal_folder", request_id: modal.request_id });
            }}><FolderOpen size={16} />{tr("Choose folder…", "Elegir carpeta…")}</button>
            <small>{path || tr("No folder selected", "Ninguna carpeta seleccionada")}</small>
          </div>}
          <label>{tr("Project name", "Nombre del proyecto")}{mode !== "empty" && <span className="project-modal-optional"> · {tr("optional", "opcional")}</span>}
            <input data-initial-focus required={mode === "empty"} value={name}
              placeholder={tr("My project", "Mi proyecto")} onChange={event => setName(event.target.value)} />
          </label>
          <div className="project-modal-destination">
            <span>{mode === "existing" ? tr("Location", "Ubicación") : tr("Destination", "Destino")}</span>
            <small>{mode === "existing" ? (path || "—") : modal.projects_root}</small>
            {mode === "existing" && <small>{tr("Your folder stays in its current location.", "Tu carpeta se mantiene en su ubicación actual.")}</small>}
          </div>
        </fieldset>
        {error && <p className="app-modal__error" role="alert">{error}</p>}
        <footer>
          <button type="button" className="app-modal__cancel" disabled={pending} onClick={dismiss}>{modal.cancel_label}</button>
          <button type="submit" className="app-modal__primary" disabled={!valid || pending}>
            {pending ? tr("Please wait…", "Un momento…") : modal.confirm_label}
          </button>
        </footer>
      </form> : <>
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

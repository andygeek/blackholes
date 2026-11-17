import { useEffect, useLayoutEffect, useReducer, useRef, useState } from "react";
import type { RefObject, TextareaHTMLAttributes } from "react";
import { ArrowUpRight, Layers, Plus, X } from "lucide-react";
import { postNative } from "../shared/native";
import { GroupIcon } from "../shared/GroupIcon";
import { TerminalProviderIcon } from "../shared/TerminalProviderIcon";
import { GroupIconPicker } from "./GroupIconPicker";
import { ModalSelect } from "./ModalSelect";
import { draftChanged, editGroupDraft, getGroupDraft, incompleteDraft, receiveGroupSnapshot, reloadGroupDraft, retryGroupSave, saveGroupDraft, type GroupLink, type TaskMetadata } from "./groupDetailsState";
export { receiveTaskDetailsSave } from "./groupDetailsState";
export type { TaskMetadata } from "./groupDetailsState";

type Language = "en" | "es";
type Terminal = { id: string; label: string; agent: string };
export type TaskDetailsData = { language: Language; id: string; workspace_id: string; project: string; details: TaskMetadata; revision: string; legacy_note?: string | null; repositories: { id: string; name: string; branch: string }[]; terminals: Terminal[] };
export type ProjectOverviewData = { language: Language; id: string; title: string; icon: string; color: string; colors: { id: string; css: string }[]; terminals: Terminal[]; tasks: { id: string; title: string; icon: string }[]; repositories: { id: string; name: string }[] };
const text = (language: Language, en: string, es: string) => language === "en" ? en : es;
const requestedFocus = new Set<string>();
/** Focus the title of a group or project page, also when the page opens after the request. */
export function requestTitleEdit(id: string) {
  requestedFocus.add(id);
  window.dispatchEvent(new CustomEvent("blackholes:focus-group-title", { detail: id }));
}
function useRequestedTitleFocus(id: string, title: RefObject<HTMLTextAreaElement | null>) {
  useEffect(() => {
    const focus = (event?: Event) => {
      if (event && (event as CustomEvent<string>).detail !== id) return;
      if (requestedFocus.delete(id)) title.current?.focus();
    };
    focus();
    window.addEventListener("blackholes:focus-group-title", focus);
    return () => window.removeEventListener("blackholes:focus-group-title", focus);
  }, [id, title]);
}
function linkName(link: GroupLink) {
  try {
    const url = new URL(link.url);
    const pr = url.pathname.match(/\/(?:pull|merge_requests)\/(\d+)/);
    if (pr) return `${url.pathname.split("/").filter(Boolean)[1] || url.hostname} #${pr[1]}`;
    return url.hostname.replace(/^www\./, "");
  } catch { return link.url; }
}

function SheetText({ inputRef, ...props }: TextareaHTMLAttributes<HTMLTextAreaElement> & { inputRef?: RefObject<HTMLTextAreaElement | null> }) {
  const localRef = useRef<HTMLTextAreaElement>(null);
  const ref = inputRef || localRef;
  useLayoutEffect(() => {
    const resize = () => { if (ref.current) { ref.current.style.height = "0"; ref.current.style.height = `${ref.current.scrollHeight}px`; } };
    resize();
    let width = ref.current?.parentElement?.clientWidth;
    const observer = new ResizeObserver(entries => {
      const next = entries[0]?.contentRect.width;
      if (next !== width) { width = next; resize(); }
    });
    if (ref.current?.parentElement) observer.observe(ref.current.parentElement);
    return () => observer.disconnect();
  }, [props.value, ref]);
  return <textarea {...props} ref={ref} rows={1} />;
}

function Sessions({ terminals, language, collapsible = false }: { terminals: Terminal[]; language: Language; collapsible?: boolean }) {
  const content = terminals.length ? <div className="task-session-list">{terminals.map(terminal => <button key={terminal.id} type="button" onClick={() => postNative({ type: "focus_terminal", terminal_id: terminal.id })}>
    <TerminalProviderIcon provider={terminal.agent} /><span>{terminal.label}</span>
  </button>)}</div> : <p>{text(language, "Start an agent from the task’s + menu.", "Inicia un agente desde el menú + de la tarea.")}</p>;
  return collapsible ? <details className="task-related group-repositories"><summary>{text(language, "Sessions", "Sesiones")} <span>{terminals.length}</span></summary>{content}</details> : <section className="task-related"><h2>{text(language, "Sessions", "Sesiones")}</h2>{content}</section>;
}

export function TaskDetails({ data }: { data: TaskDetailsData }) {
  const [, redraw] = useReducer(value => value + 1, 0);
  const titleInput = useRef<HTMLTextAreaElement>(null);
  const current = getGroupDraft(data.id, data.details, data.revision);
  const dirty = draftChanged(current);
  const incomplete = incompleteDraft(current);
  const t = (en: string, es: string) => text(data.language, en, es);
  useEffect(() => {
    if (receiveGroupSnapshot(current, data.details, data.revision)) redraw();
  }, [current, data.details, data.revision]);
  useRequestedTitleFocus(data.id, titleInput);
  useEffect(() => {
    const update = () => redraw();
    window.addEventListener("blackholes:task-details-saved", update);
    return () => {
      window.removeEventListener("blackholes:task-details-saved", update);
      saveGroupDraft(data.id);
    };
  }, [data.id]);
  const edit = (values: Partial<TaskMetadata>) => { editGroupDraft(data.id, current, values); redraw(); };
  const editLink = (index: number, values: Partial<GroupLink>) => edit({ links: current.values.links.map((link, i) => i === index ? { ...link, ...values } : link) });
  const status = current.pending ? t("Saving…", "Guardando…") : current.remote ? t("Updated elsewhere", "Actualizado en otra sesión") : current.error ? t("Could not save", "No se pudo guardar") : incomplete && dirty ? (incomplete === "title" ? t("Add a title to save", "Agrega un título para guardar") : t("Complete or remove the empty/invalid link", "Completa o quita el enlace vacío o inválido")) : dirty ? t("Unsaved changes", "Cambios sin guardar") : t("Saved", "Guardado");
  return <main className="workspace-page task-details-page" onBlurCapture={event => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) saveGroupDraft(data.id);
  }} onKeyDown={event => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") { event.preventDefault(); event.stopPropagation(); saveGroupDraft(data.id); }
  }}><div className="task-details-content">
    <header className="group-page-bar"><span><Layers size={15} />{data.project}<span className="group-breadcrumb">/ {t("Task", "Tarea")}</span></span><span className="group-save-status" data-warning={Boolean(current.error || current.remote || (dirty && incomplete)) || undefined} role="status" aria-live="polite">{status}</span></header>
    <article className="group-sheet">
      <GroupIconPicker value={current.values.icon} language={data.language} onChange={icon => edit({ icon })} />
      <SheetText inputRef={titleInput} className="group-title-input" required maxLength={500} aria-label={t("Title", "Título")} placeholder={t("Untitled task", "Tarea sin título")} value={current.values.title} onChange={event => edit({ title: event.target.value.replace(/[\r\n]+/g, " ") })} />
      <SheetText className="group-document-input" maxLength={200100} aria-label={t("Current context", "Contexto actual")} placeholder={t("Write a short summary, then the context that matters now…", "Escribe un resumen breve y luego el contexto que importa ahora…")} value={current.values.description || ""} onChange={event => edit({ description: event.target.value })} />
    </article>
    {current.remote && <aside className="group-save-warning" role="alert"><p>{t("An agent updated this task. Your draft is preserved. Review the latest version before continuing; copy any draft text you want to keep before reloading.", "Un agente actualizó esta tarea. Conservamos tu borrador. Revisa la versión actual; copia el texto que quieras mantener antes de recargar.")}</p><details><summary>{t("Latest version", "Versión actual")}</summary><strong>{current.remote.values.title}</strong><pre>{current.remote.values.description}</pre>{current.remote.values.links.map((link, i) => <p key={i}>{link.url}</p>)}</details><button type="button" disabled={Boolean(current.pending)} onClick={() => reloadGroupDraft(data.id, current, data.details, data.revision)}>{t("Discard draft and reload", "Descartar borrador y recargar")}</button></aside>}
    {current.error && <aside className="group-save-warning" role="alert"><p>{current.error}</p>{!current.remote && <button type="button" onClick={() => retryGroupSave(data.id, current)}>{t("Retry save", "Reintentar guardado")}</button>}</aside>}
    <section className="group-link-editor"><header><h2>{t("Links", "Enlaces")}</h2><button type="button" className="group-quiet-button" disabled={current.values.links.length >= 64} onClick={() => edit({ links: [...current.values.links, { kind: "context", url: "", label: null }] })}><Plus size={14} />{t("Add link", "Agregar enlace")}</button></header>
      {!current.values.links.length && <p className="group-empty">{t("Pull requests, ClickUp, Slack or other context.", "Pull requests, ClickUp, Slack u otro contexto.")}</p>}
      <div className="group-link-list">{current.values.links.map((link, index) => <div className="group-link-row" key={index}>
        <ModalSelect label={t("Link type", "Tipo de enlace")} value={link.kind} options={[{ value: "pull_request", label: "Pull Request GitHub", description: "" }, { value: "context", label: t("Context", "Contexto"), description: "" }]} onValueChange={kind => editLink(index, { kind })} />
        <input type="url" aria-label={`${t("URL", "URL")} ${index + 1}`} maxLength={4096} placeholder="https://…" value={link.url} onChange={event => editLink(index, { url: event.target.value })} />
        <button type="button" className="group-link-action" aria-label={`${t("Open", "Abrir")} ${linkName(link)}`} title={linkName(link)} disabled={!/^https?:\/\//i.test(link.url)} onClick={() => postNative({ type: "open_url", url: link.url })}><ArrowUpRight size={15} /></button>
        <button type="button" className="group-link-action" aria-label={`${t("Remove link", "Quitar enlace")} ${index + 1}`} onClick={() => edit({ links: current.values.links.filter((_, i) => i !== index) })}><X size={15} /></button>
      </div>)}</div>
    </section>
    <Sessions terminals={data.terminals} language={data.language} collapsible />
    <details className="task-related group-repositories"><summary>{t("Worktrees", "Worktrees")} <span>{data.repositories.length}</span></summary><ul>{data.repositories.map(repo => <li key={repo.id}><span>{repo.name}</span><code>{repo.branch}</code></li>)}</ul><div className="task-related-actions"><button onClick={() => postNative({ type: "add_task_repositories", task_id: data.id })}>{t("Add repositories…", "Agregar repositorios…")}</button><button onClick={() => postNative({ type: "remove_task_repositories", task_id: data.id })}>{t("Remove repositories…", "Quitar repositorios…")}</button></div></details>
    {data.legacy_note?.trim() && <details className="task-legacy"><summary>{t("Previous notes", "Notas anteriores")}</summary><pre>{data.legacy_note}</pre></details>}
  </div></main>;
}

export function ProjectOverview({ data }: { data: ProjectOverviewData }) {
  const t = (en: string, es: string) => text(data.language, en, es);
  const titleInput = useRef<HTMLTextAreaElement>(null);
  const [title, setTitle] = useState(data.title);
  // Each change is saved at once; the page then receives what was stored.
  const update = (values: { name?: string; icon?: string; color?: string }) => postNative({ type: "update_project", workspace_id: data.id, ...values });
  const saveTitle = () => {
    const name = title.trim();
    if (!name) setTitle(data.title);
    else if (name !== data.title) update({ name });
  };
  useEffect(() => setTitle(data.title), [data.title]);
  useRequestedTitleFocus(data.id, titleInput);
  return <main className="workspace-page task-details-page"><div className="task-details-content">
    <article className="group-sheet">
      <GroupIconPicker value={data.icon || "layers"} language={data.language} color={data.colors.find(color => color.id === data.color)?.css} onChange={icon => update({ icon })} />
      <SheetText inputRef={titleInput} className="group-title-input" required aria-label={t("Project name", "Nombre del proyecto")} placeholder={t("Project name", "Nombre del proyecto")} value={title} onChange={event => setTitle(event.target.value.replace(/[\r\n]+/g, " "))} onBlur={saveTitle} onKeyDown={event => {
        if (event.key === "Enter" && !event.nativeEvent.isComposing) { event.preventDefault(); event.currentTarget.blur(); }
      }} />
      <div className="project-colors" role="group" aria-label={t("Project color", "Color del proyecto")}>{data.colors.map(color => <button key={color.id} type="button" aria-label={color.id} title={color.id} aria-pressed={data.color === color.id} style={{ background: color.css }} onClick={() => update({ color: color.id })} />)}</div>
    </article>
    <Sessions terminals={data.terminals} language={data.language} />
    <section className="task-related"><h2>{t("Tasks", "Tareas")}</h2><div className="task-session-list">{data.tasks.map(task => <button key={task.id} onClick={() => postNative({ type: "open_task_details", workspace_id: data.id, task_id: task.id })}><GroupIcon value={task.icon || "layers"} size={16} /><span>{task.title}</span></button>)}</div>{!data.tasks.length && <p>{t("Create a task from the project’s + menu.", "Crea una tarea desde el menú + del proyecto.")}</p>}</section>
    <section className="task-related"><h2>{t("Repositories", "Repositorios")}</h2><div className="task-session-list">{data.repositories.map(repo => <button key={repo.id} onClick={() => postNative({ type: "open_project_repository", workspace_id: data.id, repository_id: repo.id })}>{repo.name}</button>)}</div></section>
  </div></main>;
}

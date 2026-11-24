import { useEffect, useLayoutEffect, useReducer, useRef, useState } from "react";
import type { RefObject, TextareaHTMLAttributes } from "react";
import { Archive, ArrowUpRight, Layers, Plus, Search, Trash2, X } from "lucide-react";
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
type Decision = { id: string; text: string; url?: string | null; created_at: string };
type ArchivedTask = {
  id: string; title: string; icon: string; archived_at: string; description?: string | null;
  links: GroupLink[]; decisions: Decision[]; repositories: { name?: string | null; branch: string }[];
  sessions: { agent: string; sessionId: string }[];
};
export type TaskDetailsData = { language: Language; id: string; workspace_id: string; project: string; details: TaskMetadata; revision: string; legacy_note?: string | null; decisions?: Decision[]; repositories: { id: string; name: string; branch: string }[]; terminals: Terminal[] };
export type ProjectOverviewData = { language: Language; id: string; title: string; icon: string; color: string; colors: { id: string; css: string }[]; terminals: Terminal[]; tasks: { id: string; title: string; icon: string }[]; archived?: ArchivedTask[]; repositories: { id: string; name: string }[] };
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

const formatDate = (value: string, language: Language) => {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value
    : date.toLocaleDateString(language === "en" ? "en-US" : "es-CL", { day: "numeric", month: "short", year: "numeric" });
};

function DecisionItems({ decisions, language, onRemove }: { decisions: Decision[]; language: Language; onRemove?(decision: Decision): void }) {
  return <ul className="group-decision-list">{decisions.map(decision => <li key={decision.id}>
    <time dateTime={decision.created_at}>{formatDate(decision.created_at, language)}</time>
    <span className="group-decision-text">{decision.text}</span>
    {decision.url && <button type="button" className="group-link-action" aria-label={text(language, "Open where it was decided", "Abrir dónde se decidió")}
      title={decision.url} onClick={() => postNative({ type: "open_url", url: decision.url })}><ArrowUpRight size={15} /></button>}
    {onRemove && <button type="button" className="group-link-action" aria-label={text(language, "Remove decision", "Quitar decisión")}
      title={text(language, "Remove decision", "Quitar decisión")} onClick={() => onRemove(decision)}><X size={15} /></button>}
  </li>)}</ul>;
}

/** Lasting decisions survive closing: the task is archived with them. */
function Decisions({ taskId, decisions, language }: { taskId: string; decisions: Decision[]; language: Language }) {
  const t = (en: string, es: string) => text(language, en, es);
  const [draft, setDraft] = useState("");
  const [url, setUrl] = useState("");
  const add = () => {
    const value = draft.trim();
    if (!value) return;
    postNative({ type: "add_task_decision", task_id: taskId, text: value, url: url.trim() || null });
    setDraft("");
    setUrl("");
  };
  return <section className="group-link-editor group-decisions">
    <header><h2>{t("Decisions", "Decisiones")}{decisions.length > 0 && <span className="group-count">{decisions.length}</span>}</h2></header>
    {!decisions.length && <p className="group-empty">{t("What gets decided here stays with the task, even after you close it.", "Lo que se decida aquí queda guardado en la tarea, aunque la cierres.")}</p>}
    <DecisionItems decisions={decisions} language={language}
      onRemove={decision => postNative({ type: "remove_task_decision", task_id: taskId, decision_id: decision.id })} />
    <form className="group-decision-form" onSubmit={event => { event.preventDefault(); add(); }}>
      <input aria-label={t("New decision", "Nueva decisión")} placeholder={t("Record a decision…", "Registrar una decisión…")} maxLength={2000}
        value={draft} onChange={event => setDraft(event.target.value)} />
      <input type="url" aria-label={t("Link where it was decided (optional)", "Enlace donde se decidió (opcional)")} placeholder={t("https://… (optional)", "https://… (opcional)")} maxLength={4096}
        value={url} onChange={event => setUrl(event.target.value)} />
      <button type="submit" className="group-quiet-button" disabled={!draft.trim()}><Plus size={14} />{t("Add", "Agregar")}</button>
    </form>
  </section>;
}

function ArchivedTaskItem({ task, language }: { task: ArchivedTask; language: Language }) {
  const t = (en: string, es: string) => text(language, en, es);
  const [confirming, setConfirming] = useState(false);
  useEffect(() => {
    if (!confirming) return;
    const timer = window.setTimeout(() => setConfirming(false), 5000);
    return () => window.clearTimeout(timer);
  }, [confirming]);
  return <details className="archived-task">
    <summary>
      <GroupIcon value={task.icon || "layers"} size={16} />
      <span className="archived-task__title">{task.title}</span>
      <span className="archived-task__meta">{task.decisions.length > 0 && `${task.decisions.length} ${task.decisions.length === 1 ? t("decision", "decisión") : t("decisions", "decisiones")} · `}{formatDate(task.archived_at, language)}</span>
    </summary>
    <div className="archived-task__body">
      {task.description?.trim() ? <p className="archived-task__description">{task.description}</p> : <p className="group-empty">{t("No context was written before closing.", "No se escribió contexto antes de cerrar.")}</p>}
      {task.decisions.length > 0 && <><h3>{t("Decisions", "Decisiones")}</h3><DecisionItems decisions={task.decisions} language={language} /></>}
      {task.links.length > 0 && <><h3>{t("Links", "Enlaces")}</h3><ul className="archived-task__links">{task.links.map(link => <li key={link.url}>
        <button type="button" className="archived-task__link" title={link.url} onClick={() => postNative({ type: "open_url", url: link.url })}><ArrowUpRight size={14} />{linkName(link)}</button>
      </li>)}</ul></>}
      {task.repositories.length > 0 && <><h3>{t("Branches", "Ramas")}</h3><ul className="archived-task__plain">{task.repositories.map((repo, index) => <li key={index}><span>{repo.name || t("Repository", "Repositorio")}</span><code>{repo.branch}</code></li>)}</ul></>}
      {task.sessions.length > 0 && <><h3>{t("Agent sessions", "Sesiones de agentes")}</h3><ul className="archived-task__plain">{task.sessions.map(session => <li key={session.sessionId}>
        <span className="archived-task__session"><TerminalProviderIcon provider={session.agent} />{session.agent}</span><code>{session.sessionId}</code>
      </li>)}</ul></>}
      <div className="archived-task__actions">
        <button type="button" className={`group-quiet-button archived-task__delete${confirming ? " is-confirming" : ""}`}
          onClick={() => confirming ? postNative({ type: "delete_archived_task", task_id: task.id }) : setConfirming(true)}>
          <Trash2 size={14} />{confirming ? t("Click again to delete permanently", "Haz clic de nuevo para eliminarla para siempre") : t("Delete from archive", "Eliminar del archivo")}
        </button>
      </div>
    </div>
  </details>;
}

function ArchivedTasks({ tasks, language }: { tasks: ArchivedTask[]; language: Language }) {
  const t = (en: string, es: string) => text(language, en, es);
  const [query, setQuery] = useState("");
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const visible = terms.length ? tasks.filter(task => {
    const haystack = [task.title, task.description || "", ...task.decisions.map(d => `${d.text} ${d.url || ""}`),
      ...task.links.map(link => `${link.url} ${link.label || ""}`), ...task.repositories.map(repo => `${repo.name || ""} ${repo.branch}`)].join(" ").toLowerCase();
    return terms.every(term => haystack.includes(term));
  }) : tasks;
  return <section className="task-related archived-tasks">
    <h2><Archive size={14} />{t("Archived", "Archivadas")} <span className="group-count">{tasks.length}</span></h2>
    {tasks.length > 4 && <label className="archived-search"><Search size={14} aria-hidden="true" />
      <input type="search" value={query} onChange={event => setQuery(event.target.value)}
        aria-label={t("Search archived tasks", "Buscar en tareas archivadas")} placeholder={t("Search title, context, decisions, links…", "Buscar título, contexto, decisiones, enlaces…")} /></label>}
    <div className="archived-list">{visible.map(task => <ArchivedTaskItem key={task.id} task={task} language={language} />)}</div>
    {terms.length > 0 && !visible.length && <p>{t("No archived task matches.", "Ninguna tarea archivada coincide.")}</p>}
  </section>;
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
    <Decisions taskId={data.id} decisions={data.decisions || []} language={data.language} />
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
    {(data.archived?.length || 0) > 0 && <ArchivedTasks tasks={data.archived || []} language={data.language} />}
    <section className="task-related"><h2>{t("Repositories", "Repositorios")}</h2><div className="task-session-list">{data.repositories.map(repo => <button key={repo.id} onClick={() => postNative({ type: "open_project_repository", workspace_id: data.id, repository_id: repo.id })}>{repo.name}</button>)}</div></section>
  </div></main>;
}

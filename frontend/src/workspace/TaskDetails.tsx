import { useEffect, useReducer } from "react";
import { ExternalLink, GitPullRequest, Link, ListTodo, Save } from "lucide-react";
import { createId, postNative } from "../shared/native";
import { TerminalProviderIcon } from "../shared/TerminalProviderIcon";

type Language = "en" | "es";
type Terminal = { id: string; label: string; agent: string };
export type TaskMetadata = { title: string; description: string | null; acceptanceCriteria: string | null; pullRequestUrl: string | null; externalTaskUrl: string | null };
export type TaskDetailsData = { language: Language; id: string; workspace_id: string; project: string; details: TaskMetadata; revision: string; legacy_note?: string | null; repositories: { id: string; name: string; branch: string }[]; terminals: Terminal[] };
export type ProjectOverviewData = { language: Language; id: string; title: string; terminals: Terminal[]; tasks: { id: string; title: string }[]; repositories: { id: string; name: string }[] };
type Draft = { values: TaskMetadata; base: TaskMetadata; revision: string; pending?: string; error?: string };
const drafts = new Map<string, Draft>();
const keys = ["title", "description", "acceptanceCriteria", "pullRequestUrl", "externalTaskUrl"] as const;
const copy = (metadata: TaskMetadata): TaskMetadata => Object.fromEntries(keys.map(key => [key, metadata[key]])) as TaskMetadata;
const changed = (draft: Draft) => keys.some(key => (draft.values[key] || "") !== (draft.base[key] || ""));
const text = (language: Language, en: string, es: string) => language === "en" ? en : es;

// Save acknowledgements also update drafts while another surface is visible.
export function receiveTaskDetailsSave(event: { task_id: string; request_id: string; error?: string; details?: TaskMetadata; revision?: string }) {
  const draft = drafts.get(event.task_id);
  if (!draft || draft.pending !== event.request_id) return;
  draft.pending = undefined;
  draft.error = event.error;
  if (!event.error && event.details && event.revision) {
    draft.values = copy(event.details);
    draft.base = copy(event.details);
    draft.revision = event.revision;
    postNative({ type: "task_details_dirty", task_id: event.task_id, dirty: false });
  }
  window.dispatchEvent(new Event("blackholes:task-details-saved"));
}

function Sessions({ terminals, language }: { terminals: Terminal[]; language: Language }) {
  return <section className="task-related"><h2>{text(language, "Sessions", "Sesiones")}</h2>
    {terminals.length ? <div className="task-session-list">{terminals.map(terminal => <button key={terminal.id} type="button" onClick={() => postNative({ type: "focus_terminal", terminal_id: terminal.id })}>
      <TerminalProviderIcon provider={terminal.agent} /><span>{terminal.label}</span>
    </button>)}</div> : <p>{text(language, "Use the + menu in the sidebar to start a terminal, Claude or Codex.", "Usa el menú + de la barra lateral para abrir una terminal, Claude o Codex.")}</p>}
  </section>;
}

export function TaskDetails({ data }: { data: TaskDetailsData }) {
  const [, redraw] = useReducer(value => value + 1, 0);
  let draft = drafts.get(data.id);
  if (!draft) {
    draft = { values: copy(data.details), base: copy(data.details), revision: data.revision };
    drafts.set(data.id, draft);
  }
  const current = draft;
  const dirty = changed(current);
  const conflict = current.revision !== data.revision && dirty;
  const t = (en: string, es: string) => text(data.language, en, es);
  useEffect(() => {
    if (!changed(current) && !current.pending && current.revision !== data.revision) {
      current.values = copy(data.details); current.base = copy(data.details); current.revision = data.revision;
      redraw();
    }
  }, [data.revision, current]);
  useEffect(() => {
    const update = () => redraw();
    window.addEventListener("blackholes:task-details-saved", update);
    return () => window.removeEventListener("blackholes:task-details-saved", update);
  }, []);
  const edit = (key: keyof TaskMetadata, value: string) => {
    current.values = { ...current.values, [key]: value };
    current.error = undefined;
    postNative({ type: "task_details_dirty", task_id: data.id, dirty: changed(current) });
    redraw();
  };
  const save = () => {
    if (current.pending || !changed(current) || !current.values.title.trim()) return;
    current.pending = createId(); current.error = undefined;
    const patch = Object.fromEntries(keys.filter(key => (current.values[key] || "") !== (current.base[key] || "")).map(key => [key, current.values[key] || null]));
    postNative({ type: "save_task_details", task_id: data.id, request_id: current.pending, patch: { ...patch, expectedRevision: current.revision } });
    redraw();
  };
  const reset = () => {
    current.values = copy(data.details); current.base = copy(data.details); current.revision = data.revision; current.error = undefined;
    postNative({ type: "task_details_dirty", task_id: data.id, dirty: false }); redraw();
  };
  return <main className="workspace-page task-details-page"><div className="task-details-content">
    <header className="task-details-heading"><span><ListTodo size={23} /></span><div><p>{data.project}</p><h1>{t("Task details", "Detalles de la tarea")}</h1></div></header>
    <form onSubmit={event => { event.preventDefault(); save(); }} onKeyDown={event => { if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") { event.preventDefault(); save(); } }}>
      <fieldset disabled={Boolean(current.pending)}>
        <label>{t("Title", "Título")}<input required maxLength={500} value={current.values.title} onChange={event => edit("title", event.target.value)} /></label>
        <label>{t("Objective and description", "Objetivo y descripción")}<textarea rows={5} maxLength={100000} value={current.values.description || ""} placeholder={t("What should this task accomplish?", "¿Qué debe lograr esta tarea?")} onChange={event => edit("description", event.target.value)} /></label>
        <label>{t("Acceptance criteria", "Criterios de aceptación")}<textarea rows={5} maxLength={100000} value={current.values.acceptanceCriteria || ""} placeholder={t("Conditions that must be met to consider the task complete.", "Condiciones que deben cumplirse para dar la tarea por terminada.")} onChange={event => edit("acceptanceCriteria", event.target.value)} /></label>
        <div className="task-link-fields">{([['pullRequestUrl', t('Pull request', 'Pull request'), GitPullRequest], ['externalTaskUrl', t('External task', 'Tarea externa'), Link]] as const).map(([key, label, Icon]) => <label key={key}><span><Icon size={15} />{label}</span><div className="task-url-input"><input type="url" maxLength={4096} pattern="https?://.*" placeholder="https://…" value={current.values[key] || ""} onChange={event => edit(key, event.target.value)} />{current.values[key] && /^https?:\/\//i.test(current.values[key]!) && <button type="button" title={t("Open link", "Abrir enlace")} aria-label={`${t("Open", "Abrir")} ${label}`} onClick={() => postNative({ type: "open_url", url: current.values[key] })}><ExternalLink size={16} /></button>}</div></label>)}</div>
      </fieldset>
      {conflict && <p className="task-details-warning" role="status">{t("These details changed elsewhere. Your draft is preserved. Copy anything you want to keep before reloading.", "Estos datos cambiaron en otra sesión. Tu borrador está conservado. Copia lo que quieras mantener antes de recargar.")}</p>}
      {current.error && <p className="task-details-warning" role="alert">{current.error}</p>}
      <footer className="task-details-actions"><span aria-live="polite">{current.pending ? t("Saving…", "Guardando…") : dirty ? t("Unsaved changes", "Cambios sin guardar") : t("Saved", "Guardado")}</span>
        {(dirty || current.error) && <button type="button" disabled={Boolean(current.pending)} onClick={reset}>{conflict ? t("Discard draft and reload", "Descartar borrador y recargar") : t("Discard changes", "Descartar cambios")}</button>}
        <button type="submit" className="task-save" disabled={!dirty || Boolean(current.pending) || conflict || !current.values.title.trim()}><Save size={15} />{t("Save changes", "Guardar cambios")}</button>
      </footer>
    </form>
    <Sessions terminals={data.terminals} language={data.language} />
    <section className="task-related"><h2>{t("Repositories", "Repositorios")}</h2><ul>{data.repositories.map(repo => <li key={repo.id}><span>{repo.name}</span><code>{repo.branch}</code></li>)}</ul><div className="task-related-actions"><button onClick={() => postNative({ type: "add_task_repositories", task_id: data.id })}>{t("Add repositories…", "Agregar repositorios…")}</button><button onClick={() => postNative({ type: "remove_task_repositories", task_id: data.id })}>{t("Remove repositories…", "Quitar repositorios…")}</button></div></section>
    {data.legacy_note?.trim() && <details className="task-legacy"><summary>{t("Previous notes", "Notas anteriores")}</summary><p>{t("Preserved as additional context. Copy relevant content into the fields above.", "Conservadas como contexto adicional. Copia el contenido relevante en los campos de arriba.")}</p><pre>{data.legacy_note}</pre></details>}
  </div></main>;
}

export function ProjectOverview({ data }: { data: ProjectOverviewData }) {
  const t = (en: string, es: string) => text(data.language, en, es);
  return <main className="workspace-page task-details-page"><div className="task-details-content"><h1>{data.title}</h1>
    <Sessions terminals={data.terminals} language={data.language} />
    <section className="task-related"><h2>{t("Tasks", "Tareas")}</h2><div className="task-session-list">{data.tasks.map(task => <button key={task.id} onClick={() => postNative({ type: "open_task_details", workspace_id: data.id, task_id: task.id })}><ListTodo size={16} /><span>{task.title}</span></button>)}</div>{!data.tasks.length && <p>{t("Create a task from the project’s + menu.", "Crea una tarea desde el menú + del proyecto.")}</p>}</section>
    <section className="task-related"><h2>{t("Repositories", "Repositorios")}</h2><div className="task-session-list">{data.repositories.map(repo => <button key={repo.id} onClick={() => postNative({ type: "open_project_repository", workspace_id: data.id, repository_id: repo.id })}>{repo.name}</button>)}</div></section>
  </div></main>;
}

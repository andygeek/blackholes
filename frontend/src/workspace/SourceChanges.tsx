import { useEffect, useState } from "react";
import { Check, ChevronDown, ChevronRight, FileCode2, LoaderCircle, Minus, Plus } from "lucide-react";
import { createId, postNative } from "../shared/native";
import type { ChangeRow, ExplorerData } from "./WorkspaceSurface";

export interface IndexInfo { token: string; head: string | null; branch: string | null; blocked: string | null }
const drafts = new Map<string, string>();
const pending = new Map<string, { id: string; message: string; operation: string }>();
const feedback = new Map<string, { error?: string; commit?: string }>();
export function receiveSourceControlResult(event: { root: string; request_id: string; operation: string; error?: string; commit?: string }) {
  const action = pending.get(event.root);
  if (action?.id !== event.request_id) return;
  pending.delete(event.root);
  if (!event.error && event.operation === "commit" && drafts.get(event.root) === action.message) drafts.delete(event.root);
  feedback.set(event.root, { error: event.error, commit: event.commit });
  window.dispatchEvent(new CustomEvent("blackholes:source-control-result", { detail: event }));
}
const marker = { added: "A", deleted: "D", modified: "M", renamed: "R", untracked: "U", conflicted: "!" };

export function SourceChanges({ explorer, language }: { explorer: ExplorerData; language: "en" | "es" }) {
  const root = explorer.root_path;
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const [expanded, setExpanded] = useState(true);
  const [message, setMessage] = useState(() => drafts.get(root) || "");
  const [, rerender] = useState(0);
  useEffect(() => {
    const receive = (event: Event) => {
      if ((event as CustomEvent).detail.root !== root) return;
      setMessage(drafts.get(root) || ""); rerender(n => n + 1);
    };
    window.addEventListener("blackholes:source-control-result", receive);
    return () => window.removeEventListener("blackholes:source-control-result", receive);
  }, [root]);
  const staged = explorer.changes.filter(c => c.staged_kind);
  const unstaged = explorer.changes.filter(c => c.unstaged_kind);
  const busy = !!explorer.history?.busy || pending.has(root);
  const unavailable = busy || !explorer.index || !!explorer.index.blocked || explorer.changes_state !== "ready";
  const canCommit = !unavailable && staged.length > 0 && !!message.trim() && !explorer.changes.some(c => c.kind === "conflicted");
  const response = feedback.get(root);
  const act = (operation: "stage" | "unstage" | "commit", relative_path?: string) => {
    if (unavailable || !explorer.index || (operation === "commit" && !canCommit)) return;
    const id = createId();
    pending.set(root, { id, message, operation }); feedback.delete(root); rerender(n => n + 1);
    postNative({ type: "source_control_action", root_path: root, request_id: id, operation, token: explorer.index.token, relative_path, message });
  };
  return <section className={`repository-files-section scm-section${expanded ? "" : " is-collapsed"}`}>
    <button className="repository-changes-toggle" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>{expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}<strong>{tr("Changes", "Cambios")}</strong><small>{explorer.changes.length}</small></button>
    {expanded && <>
      <div className="scm-composer">
        <textarea aria-label={tr("Commit message", "Mensaje del commit")} placeholder={tr("Message (⌘Enter to commit staged changes)", "Mensaje (⌘Enter para crear el commit)")}
          rows={3} maxLength={65536} value={message} readOnly={pending.get(root)?.operation === "commit"}
          onChange={e => { setMessage(e.target.value); drafts.set(root, e.target.value); }}
          onKeyDown={e => { if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); act("commit"); } }} />
        <button className="scm-commit-button" disabled={!canCommit} onClick={() => act("commit")}>
          {pending.get(root)?.operation === "commit" ? <LoaderCircle size={14} className="git-spinning" /> : <Check size={14} />}
          {pending.get(root)?.operation === "commit" ? tr("Committing…", "Creando commit…") : tr("Commit", "Crear commit")}{staged.length > 0 && <small>{staged.length}</small>}
        </button>
        {!staged.length && <p className="scm-help">{tr("Use + to choose files for this commit.", "Usa + para elegir los archivos del commit.")}</p>}
        {explorer.index?.blocked && <p className="scm-feedback is-error">{explorer.index.blocked}</p>}
        {response?.error && <p className="scm-feedback is-error" role="alert">{response.error}</p>}
        {response?.commit && <p className="scm-feedback" role="status">{tr("Committed", "Commit creado")} <code>{response.commit.slice(0, 8)}</code></p>}
      </div>
      <div className="scm-files-scroll">
        <div className="repository-root" title={root}><strong>{explorer.root_label}</strong></div>
        {explorer.changes_state === "error" ? <div className="explorer-state is-error">{explorer.changes_error}</div> : <>
          <ChangeGroup label={tr("Staged Changes", "Cambios preparados")} rows={staged} staged root={root} language={language} disabled={unavailable} onAction={path => act("unstage", path)} />
          <ChangeGroup label={tr("Changes", "Cambios pendientes")} rows={unstaged} staged={false} root={root} language={language} disabled={unavailable} onAction={path => act("stage", path)} />
          {!explorer.changes.length && <p className="git-hint">{explorer.changes_state === "loading" ? tr("Loading changes…", "Cargando cambios…") : tr("Working tree clean", "Sin cambios locales")}</p>}
        </>}
      </div>
    </>}
  </section>;
}

function ChangeGroup({ label, rows, staged, root, language, disabled, onAction }: {
  label: string; rows: ChangeRow[]; staged: boolean; root: string; language: "en" | "es"; disabled: boolean; onAction(path?: string): void;
}) {
  const [expanded, setExpanded] = useState(true);
  const [limit, setLimit] = useState(200);
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const Icon = staged ? Minus : Plus;
  const action = staged ? tr("Unstage changes", "Quitar del commit") : tr("Stage changes", "Preparar para el commit");
  return <div className="scm-group">
    <div className="scm-group-header"><button aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>{expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}<strong>{label}</strong><small>{rows.length}</small></button>
      <button className="scm-file-action" disabled={disabled || !rows.length} aria-label={staged ? tr("Unstage all", "Quitar todos del commit") : tr("Stage all", "Preparar todos")}
        title={staged ? tr("Unstage all", "Quitar todos del commit") : tr("Stage all", "Preparar todos")} onClick={() => onAction()}><Icon size={14} /></button></div>
    {expanded && rows.slice(0, limit).map(row => {
      const kind = (staged ? row.staged_kind : row.unstaged_kind)!;
      const selected = staged ? row.selected_staged : row.selected;
      return <div className={`scm-file-row${selected ? " is-selected" : ""}`} key={row.relative_path}>
        <button className="scm-file-open" aria-current={selected ? true : undefined} title={row.previous_relative_path ? `${row.previous_relative_path} → ${row.relative_path}` : row.relative_path}
          onClick={() => postNative({ type: "open_repository_diff", root_path: root, relative_path: row.relative_path, staged })}>
          <FileCode2 size={14} /><span>{row.relative_path.split("/").pop()}</span><small>{row.relative_path.includes("/") ? row.relative_path.slice(0, row.relative_path.lastIndexOf("/")) : ""}</small><b className={`change-marker is-${kind}`}>{marker[kind]}</b>
        </button><button className="scm-file-action" disabled={disabled} title={action} aria-label={`${action}: ${row.relative_path}`} onClick={() => onAction(row.relative_path)}><Icon size={14} /></button>
      </div>;
    })}
    {expanded && rows.length > limit && <button className="git-load-more" onClick={() => setLimit(n => n + 200)}>{tr("Show more files", "Mostrar más archivos")}</button>}
  </div>;
}

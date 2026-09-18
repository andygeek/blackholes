import { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, Check, ChevronDown, ChevronRight, Cloud, Copy, FileCode2, GitBranch, GitCommitHorizontal, RefreshCw, Tag, X } from "lucide-react";
import { postNative } from "../shared/native";
import type { ChangeRow } from "./WorkspaceSurface";

interface Commit { id: string; parents: string[]; author: string; timestamp: number; subject: string; refs: { name: string; kind: string }[]; outgoing: boolean }
interface Snapshot {
  commits: Commit[]; has_more: boolean; head: string | null; branch: string | null; upstream: string | null;
  ahead: number; behind: number; remotes: string[]; push_remote: string | null; push_branch: string | null;
}
interface CommitFile { relative_path: string; previous_relative_path: string | null; kind: ChangeRow["kind"] }
interface Detail { id: string; parents: string[]; parent: string | null; author: string; timestamp: number; message: string; files: CommitFile[] }
export interface HistoryData {
  snapshot: Snapshot | null; loading: boolean; error: string | null; selected: string | null; detail: Detail | null;
  detail_loading: boolean; detail_error: string | null; busy: boolean; message: string | null; selected_file: string | null;
}
const colors = ["#d69b62", "#839ddd", "#82b899", "#bd8bc3", "#64b7c7", "#d5be72"];
const marker = { added: "A", deleted: "D", modified: "M", renamed: "R", untracked: "U", conflicted: "!" };
const short = (id: string) => id.slice(0, 8);
const date = (timestamp: number, language: string) => new Date(timestamp * 1000).toLocaleString(language);

// Each row transforms the pending-parent lanes into the next row's lanes.
// Edges follow actual parent OIDs, including merges and branches already in flight.
function layout(commits: Commit[]) {
  let lanes: string[] = [];
  let colorIndex = 0;
  const palette = new Map<string, string>();
  const color = (id: string) => { if (!palette.has(id)) palette.set(id, colors[colorIndex++ % colors.length]); return palette.get(id)!; };
  return commits.map(commit => {
    const before = [...lanes];
    let lane = before.indexOf(commit.id);
    if (lane < 0) lane = before.length;
    const nodeColor = color(commit.id);
    const after = before.filter(id => id !== commit.id);
    commit.parents.forEach((parent, index) => {
      if (!after.includes(parent)) after.splice(Math.min(lane + index, after.length), 0, parent);
      if (!palette.has(parent)) palette.set(parent, index === 0 ? nodeColor : colors[colorIndex++ % colors.length]);
    });
    const edges: { from: number; to: number; start: number; end: number; color: string }[] = [];
    before.forEach((id, index) => {
      if (id === commit.id) edges.push({ from: index, to: lane, start: 0, end: 14, color: nodeColor });
      else edges.push({ from: index, to: after.indexOf(id), start: 0, end: 28, color: color(id) });
    });
    commit.parents.forEach(parent => edges.push({ from: lane, to: after.indexOf(parent), start: 14, end: 28, color: color(parent) }));
    lanes = after;
    return { lane, color: nodeColor, edges, continuation: after.map((id, lane) => ({ lane, color: color(id) })), width: Math.max(lane + 1, before.length, after.length, 1) * 12 + 12 };
  });
}

function CommitFiles({ detail, root, language, selected }: { detail: Detail; root: string; language: "en" | "es"; selected?: string | null }) {
  const [visibleCount, setVisibleCount] = useState(200);
  useEffect(() => setVisibleCount(200), [detail.id, detail.parent]);
  return <div className="git-commit-files" aria-label={language === "en" ? "Changed files in commit" : "Archivos modificados en el commit"}>
    {detail.files.slice(0, visibleCount).map(file => <button className={`git-commit-file${selected === file.relative_path ? " is-selected" : ""}`} aria-current={selected === file.relative_path ? true : undefined} key={file.relative_path}
      title={file.previous_relative_path ? `${file.previous_relative_path} → ${file.relative_path}` : file.relative_path}
      onClick={() => postNative({ type: "open_git_commit_diff", root_path: root, commit: detail.id, relative_path: file.relative_path })}>
      <FileCode2 size={13} /><span>{file.relative_path.split("/").pop()}</span><small>{file.relative_path.includes("/") ? file.relative_path.slice(0, file.relative_path.lastIndexOf("/")) : ""}</small><b className={`change-marker is-${file.kind}`}>{marker[file.kind]}</b>
    </button>)}
    {detail.files.length > visibleCount && <button className="git-load-more" onClick={() => setVisibleCount(n => n + 200)}>{language === "en" ? "Show more files" : "Mostrar más archivos"} ({detail.files.length - visibleCount})</button>}
    {!detail.files.length && <p className="git-hint">{language === "en" ? "No file changes against this parent." : "Sin cambios de archivos respecto a este padre."}</p>}
  </div>;
}

export function CommitOverview({ history, root, language }: { history: HistoryData; root: string; language: "en" | "es" }) {
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const detail = history.detail;
  if (history.detail_loading) return <div className="workspace-empty">{tr("Loading commit…", "Cargando commit…")}</div>;
  if (history.detail_error) return <div className="workspace-error">{history.detail_error}</div>;
  if (!detail) return null;
  return <section className="git-commit-overview">
    <header><GitCommitHorizontal size={22} /><h2>{tr("Commit", "Commit")} {short(detail.id)}</h2><button title={tr("Copy commit hash", "Copiar hash del commit")} aria-label={tr("Copy commit hash", "Copiar hash del commit")} onClick={() => postNative({ type: "copy_text", text: detail.id })}><Copy size={15} /></button></header>
    <p className="git-author">{detail.author} · {date(detail.timestamp, language)}</p>
    <pre>{detail.message}</pre>
    <div className="git-comparison-parent"><span>{tr("Compare with", "Comparar con")}</span>{detail.parents.length > 1
      ? <select aria-label={tr("Parent commit", "Commit padre")} value={detail.parent || ""} onChange={e => postNative({ type: "select_git_commit", root_path: root, commit: detail.id, parent: e.target.value })}>
        {detail.parents.map((parent, index) => <option key={parent} value={parent}>{tr("Parent", "Padre")} {index + 1} · {short(parent)}</option>)}
      </select> : <code>{detail.parent ? short(detail.parent) : tr("Empty tree (initial commit)", "Árbol vacío (commit inicial)")}</code>}</div>
    <h3>{detail.files.length} {tr("changed files", "archivos modificados")}</h3>
    <CommitFiles detail={detail} root={root} language={language} selected={history.selected_file} />
  </section>;
}

export function GitHistory({ history, root, language }: { history?: HistoryData; root: string; language: "en" | "es" }) {
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const [expanded, setExpanded] = useState(true);
  const [remote, setRemote] = useState("");
  const [preview, setPreview] = useState(false);
  const snapshot = history?.snapshot;
  const graph = useMemo(() => layout(snapshot?.commits || []), [snapshot?.commits]);
  const signature = `${root}:${snapshot?.head}:${snapshot?.branch}:${snapshot?.upstream}:${snapshot?.push_remote}:${snapshot?.push_branch}`;
  useEffect(() => { setPreview(false); }, [signature]);
  useEffect(() => {
    setRemote(current => snapshot?.push_remote || (snapshot?.remotes.includes(current) ? current : snapshot?.remotes.includes("origin") ? "origin" : snapshot?.remotes[0] || ""));
  }, [root, snapshot?.push_remote, snapshot?.remotes.join("\0")]);
  if (!history) return null;
  const busy = history.busy || history.loading;
  const pushRemote = snapshot?.upstream ? snapshot.push_remote : remote;
  const target = snapshot?.upstream ? snapshot.push_branch : snapshot?.branch;
  const canPush = !!(snapshot?.head && snapshot.branch && pushRemote && target && !busy && !history.error && (!snapshot.upstream || snapshot.ahead > 0));
  const refresh = () => postNative({ type: "refresh_git_history", root_path: root, load_more: false });
  const action = (operation: "fetch" | "push") => {
    postNative({ type: "git_remote_operation", root_path: root, operation, remote: operation === "push" ? pushRemote : remote,
      expected_head: snapshot?.head, expected_branch: snapshot?.branch, expected_upstream: snapshot?.upstream, expected_target: target });
    setPreview(false);
  };
  return <section className={`git-history${expanded ? " is-expanded" : ""}`}>
    <header className="git-history-header"><button className="git-section-toggle" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>{expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}<strong>{tr("Graph", "Gráfico")}</strong></button>
      <button disabled={busy || !remote || !!history.error} title={tr("Fetch remote changes", "Traer referencias del remoto")} aria-label={tr("Fetch remote changes", "Traer referencias del remoto")} onClick={() => action("fetch")}><ArrowDown size={14} /></button>
      <button disabled={!canPush} title={tr("Review push", "Revisar push")} aria-label={tr("Review push", "Revisar push")} onClick={() => setPreview(!preview)}><ArrowUp size={14} /></button>
      <button disabled={busy} title={tr("Refresh local history", "Actualizar historial local")} aria-label={tr("Refresh local history", "Actualizar historial local")} onClick={refresh}><RefreshCw size={14} className={busy ? "git-spinning" : ""} /></button>
    </header>
    {expanded && <>
      {snapshot && <div className="git-branch-summary">
        <div title={snapshot.upstream || ""}><GitBranch size={13} /><strong>{snapshot.branch || tr("Detached HEAD", "HEAD separado")}</strong><span>{snapshot.upstream ? `↑${snapshot.ahead} ↓${snapshot.behind}` : tr("No upstream", "Sin upstream")}</span></div>
        <div className="git-remote-line"><span title={tr("Based on local references. Fetch to update remote state.", "Basado en referencias locales. Usa fetch para actualizar el remoto.")}>{snapshot.upstream || tr("Not tracking a remote branch", "Sin seguimiento de rama remota")}</span>
          {snapshot.remotes.length > 1 && <select aria-label={tr("Fetch remote", "Remoto para fetch")} value={remote} disabled={busy} onChange={e => { setRemote(e.target.value); setPreview(false); }}>{snapshot.remotes.map(r => <option key={r}>{r}</option>)}</select>}
        </div>
      </div>}
      {preview && snapshot && <div className="git-push-preview">
        <strong>{tr("Push branch", "Enviar rama")}</strong><code>{snapshot.branch} → {pushRemote}/{target}</code>
        <p>{snapshot.upstream ? `${snapshot.ahead} ${tr("outgoing commits", "commits pendientes")}` : tr("Publish this branch and set its upstream.", "Publicar esta rama y configurar su upstream.")} · {short(snapshot.head!)}</p>
        {snapshot.behind > 0 && <p>{tr("Remote changes exist. Git may reject this push; reconcile with your agent first.", "Hay cambios remotos. Git puede rechazar el push; resuélvelos antes con tu agente.")}</p>}
        <div><button disabled={!canPush} onClick={() => action("push")}><ArrowUp size={13} />{tr("Push", "Enviar")}</button><button onClick={() => setPreview(false)}><X size={13} />{tr("Cancel", "Cancelar")}</button></div>
      </div>}
      {history.message && <p className="git-operation-message" role="status">{history.message}</p>}
      {history.error && <p className="git-operation-message is-error" role="alert">{history.error}</p>}
      <div className="git-history-scroll" aria-label={tr("Git commit history", "Historial de commits de Git")}>
        {!snapshot?.commits.length && <p className="git-hint">{history.loading ? tr("Loading history…", "Cargando historial…") : history.error ? "" : tr("No commits yet", "Aún no hay commits")}</p>}
        {snapshot?.commits.map((commit, index) => {
          const row = graph[index];
          const selected = history.selected === commit.id;
          return <div className="git-history-entry" key={commit.id}>
            <button className={`git-commit-row${selected ? " is-selected" : ""}`} aria-expanded={selected}
              title={`${commit.subject}\n${commit.author} · ${date(commit.timestamp, language)}\n${commit.id}`}
              onClick={() => postNative({ type: "select_git_commit", root_path: root, commit: commit.id })}>
              <svg width={row.width} height={28} aria-hidden="true" className="git-lanes">
                {row.edges.map((edge, i) => <path key={i} stroke={edge.color} fill="none" strokeWidth={1.5} d={`M ${edge.from * 12 + 10} ${edge.start} C ${edge.from * 12 + 10} ${(edge.start + edge.end) / 2}, ${edge.to * 12 + 10} ${(edge.start + edge.end) / 2}, ${edge.to * 12 + 10} ${edge.end}`} />)}
                <circle cx={row.lane * 12 + 10} cy={14} r={commit.parents.length > 1 ? 4.5 : 3.5} stroke={row.color} strokeWidth={1.5} fill={commit.parents.length > 1 ? "var(--git-node-bg)" : row.color} />
              </svg>
              <span className="git-commit-subject">{commit.subject || tr("(No message)", "(Sin mensaje)")}</span>
              {commit.id === snapshot.head && <span className="git-head" title="HEAD">●</span>}
              {commit.outgoing && <ArrowUp size={11} className="git-outgoing" aria-label={tr("Not pushed", "Sin enviar")} />}
              {commit.refs.slice(0, 2).map(ref => <span className={`git-ref is-${ref.kind}`} key={`${ref.kind}:${ref.name}`} title={ref.name}>{ref.kind === "remote" ? <Cloud size={10} /> : ref.kind === "tag" ? <Tag size={10} /> : <GitBranch size={10} />}{ref.name}</span>)}
              {commit.refs.length > 2 && <span className="git-more-refs" title={commit.refs.slice(2).map(r => r.name).join("\n")}>+{commit.refs.length - 2}</span>}
              {!commit.refs.length && <small className="git-commit-author">{commit.author}</small>}
            </button>
            {selected && <div className="git-expanded-commit" style={{ paddingLeft: row.width }}>
              <svg width={row.width} height="100%" aria-hidden="true" className="git-continuation">{row.continuation.map(line => <line key={line.lane} x1={line.lane * 12 + 10} x2={line.lane * 12 + 10} y1="0" y2="100%" stroke={line.color} strokeWidth={1.5} />)}</svg>
              {history.detail_loading ? <p className="git-hint">{tr("Loading…", "Cargando…")}</p> : history.detail_error ? <p className="git-hint is-error">{history.detail_error}</p> : history.detail && <CommitFiles detail={history.detail} root={root} language={language} selected={history.selected_file} />}
            </div>}
          </div>;
        })}
        {snapshot?.has_more && <button className="git-load-more" disabled={busy || snapshot.commits.length >= 1000} onClick={() => postNative({ type: "refresh_git_history", root_path: root, load_more: true })}>{snapshot.commits.length >= 1000 ? tr("Showing 1,000 recent commits", "Mostrando los 1000 commits recientes") : tr("Load more commits", "Cargar más commits")}</button>}
      </div>
      {snapshot && <footer className="git-history-footer">{snapshot.commits.length} {tr("commits · all branches", "commits · todas las ramas")}{!history.loading && !history.error && <Check size={11} />}</footer>}
    </>}
  </section>;
}

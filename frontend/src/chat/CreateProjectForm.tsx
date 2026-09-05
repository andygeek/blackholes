import { useEffect, useState } from "react";
import { FolderOpen, GitBranch, Plus, X } from "lucide-react";
import { postNative } from "../shared/native";
import type { AppModalState } from "./types";

type Source = { kind: "local" | "github"; value: string; name: string; selected: boolean };

export function CreateProjectForm({ modal, language, onDismiss, onBusyChange }: {
  modal: AppModalState; language: "en" | "es"; onDismiss: () => void; onBusyChange: (busy: boolean) => void;
}) {
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [showGithub, setShowGithub] = useState(false);
  const [sources, setSources] = useState<Source[]>([]);
  const [mode, setMode] = useState<"link" | "copy">("link");
  const [pending, setPending] = useState<"scan" | "create" | null>(null);
  const [error, setError] = useState("");
  const selected = sources.filter(source => source.selected);

  useEffect(() => {
    if (!modal.feedback) return;
    setPending(null);
    setError(modal.feedback.error || "");
    const repositories = modal.feedback.repositories || [];
    setSources(current => [...current, ...repositories
      .filter(repo => !current.some(source => source.kind === "local" && source.value === repo.path))
      .map(repo => ({ kind: "local" as const, value: repo.path, name: repo.name, selected: true }))]);
  }, [modal.feedback]);
  useEffect(() => { onBusyChange(pending !== null); }, [pending, onBusyChange]);

  const addGithub = () => {
    const value = url.trim().replace(/\/$/, "");
    if (!/^(?:https:\/\/github\.com\/|git@github\.com:)?[\w.-]+\/[\w.-]+(?:\.git)?$/.test(value)) {
      setError(tr("Use a GitHub URL or owner/repository.", "Usa una URL de GitHub o propietario/repositorio."));
      return;
    }
    const key = (v: string) => v.replace(/^(?:https:\/\/github\.com\/|git@github\.com:)/, "").replace(/\.git$/, "").toLowerCase();
    if (sources.some(source => source.kind === "github" && key(source.value) === key(value))) {
      setError(tr("This repository is already in the list.", "Este repositorio ya está en la lista."));
      return;
    }
    const cloneUrl = value.startsWith("https://") || value.startsWith("git@") ? value : `https://github.com/${value}`;
    setSources(current => [...current, { kind: "github", value: cloneUrl, name: key(value), selected: true }]);
    setUrl(""); setError("");
  };

  return <form className="create-project-form" onSubmit={event => {
    event.preventDefault();
    if (pending || !name.trim()) return;
    if (url.trim()) { setShowGithub(true); setError(tr("Add the GitHub repository first, or clear its URL.", "Agrega primero el repositorio de GitHub o borra su URL.")); return; }
    setError(""); setPending("create"); onBusyChange(true);
    postNative({ type: "submit_create_project", request_id: modal.request_id, name: name.trim(),
      mode, sources: selected.map(({ kind, value }) => ({ kind, value })) });
  }}>
    <p id="app-modal-description">{tr("Your repositories, together in one project.", "Tus repositorios, juntos en un proyecto.")}</p>
    <fieldset disabled={pending !== null} className="project-modal-fields">
      <legend>{tr("Project settings", "Configuración del proyecto")}</legend>
      <label>{tr("Name", "Nombre")}
        <input data-initial-focus required value={name} onChange={event => setName(event.target.value)} placeholder={tr("My project", "Mi proyecto")} />
      </label>
      <div className="project-repository-heading">
        <span>{tr("Repositories", "Repositorios")} <small>{tr("Optional", "Opcional")}</small></span>
        {sources.length > 1 && <button type="button" className="project-text-button" onClick={() => setSources(current => current.map(source => ({ ...source, selected: selected.length !== sources.length })))}>
          {selected.length === sources.length ? tr("Deselect all", "Quitar selección") : tr("Select all", "Seleccionar todos")}
        </button>}
      </div>
      <div className="project-source-actions">
        <button type="button" onClick={() => {
          setPending("scan"); onBusyChange(true); setError("");
          postNative({ type: "choose_project_modal_folder", request_id: modal.request_id });
        }}><FolderOpen size={16} />{pending === "scan" ? tr("Searching…", "Buscando…") : tr("Local folder", "Carpeta local")}</button>
        <button type="button" aria-expanded={showGithub} aria-controls="project-github-source" onClick={() => setShowGithub(!showGithub)}>
          <GitBranch size={16} />GitHub
        </button>
      </div>
      {showGithub && <div id="project-github-source">
        <label className="project-visually-hidden" htmlFor="project-github-url">GitHub URL</label>
        <div className="project-modal-github">
          <input id="project-github-url" autoFocus value={url} onChange={event => setUrl(event.target.value)} placeholder="owner/repository" spellCheck={false}
            onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); addGithub(); } }} />
          <button type="button" disabled={!url.trim()} onClick={addGithub}><Plus size={16} />{tr("Add", "Agregar")}</button>
        </div>
        <small>{tr("Downloads into the project folder.", "Se descarga dentro del proyecto.")}</small>
      </div>}
      {sources.length > 0 ? <div className="project-modal-repositories">
        {sources.map((source, index) => <div className="project-modal-repo" key={source.kind + ":" + source.value}>
          <label className="project-modal-repo-check">
            <input type="checkbox" checked={source.selected} onChange={event => {
              const checked = event.target.checked;
              setSources(current => current.map((item, i) => i === index ? { ...item, selected: checked } : item));
            }} />
            {source.kind === "local" ? <FolderOpen size={16} /> : <GitBranch size={16} />}
            <span title={source.value}><strong>{source.name}</strong><small>{source.value}</small></span>
          </label>
          <button type="button" aria-label={tr("Remove ", "Quitar ") + source.name} onClick={() => setSources(current => current.filter((_, i) => i !== index))}><X size={15} /></button>
        </div>)}
      </div> : <small className="project-empty-hint">{tr("Choose a repository or a folder with several. You can also add them later.", "Elige un repositorio o una carpeta con varios. También puedes agregarlos después.")}</small>}
      {selected.some(source => source.kind === "local") && <div className="project-location-option">
        <label htmlFor="project-repository-mode">{tr("Local repositories", "Repositorios locales")}</label>
        <select id="project-repository-mode" value={mode} onChange={event => setMode(event.target.value as "link" | "copy")}>
          <option value="link">{tr("Link (default)", "Vincular (predeterminado)")}</option>
          <option value="copy">{tr("Copy into project", "Copiar al proyecto")}</option>
        </select>
        <small>{mode === "link"
          ? tr("Shortcuts in your project. Edits affect the originals.", "Accesos directos en tu proyecto. Editas los originales.")
          : tr("Includes .env and pending changes. Originals stay untouched.", "Incluye .env y cambios pendientes. No modifica los originales.")}</small>
      </div>}
      <details className="project-details">
        <summary tabIndex={pending ? -1 : 0} onClick={event => { if (pending) event.preventDefault(); }}>{tr("Location and details", "Ubicación y detalles")}</summary>
        <small>{tr("Projects folder", "Carpeta de proyectos")}</small>
        <code>{modal.projects_root}</code>
        <p>{tr("Each project has its own instructions and notes. Local links appear directly in its folder and work without Blackholes. GitHub repositories are downloaded there.", "Cada proyecto tiene sus instrucciones y notas. Los enlaces locales aparecen directamente en su carpeta y funcionan sin Blackholes. Los repositorios de GitHub se descargan allí.")}</p>
        {mode === "copy" && <p>{tr("Copies files, Git history and dependencies. Pause running processes first. Large folders take longer; symbolic links may still point outside the copy.", "Copia archivos, historial Git y dependencias. Pausa los procesos antes de copiar. Las carpetas grandes tardan más; los enlaces simbólicos pueden seguir apuntando fuera de la copia.")}</p>}
      </details>
    </fieldset>
    {error && <p className="app-modal__error" role="alert">{error}</p>}
    <footer>
      <button type="button" className="app-modal__cancel" disabled={pending !== null} onClick={onDismiss}>{modal.cancel_label}</button>
      <button type="submit" className="app-modal__primary" disabled={pending !== null || !name.trim()}>{pending === "create" ? tr("Creating…", "Creando…") : modal.confirm_label}</button>
    </footer>
  </form>;
}

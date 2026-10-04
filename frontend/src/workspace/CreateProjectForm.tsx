import { useEffect, useRef, useState } from "react";
import { FolderOpen, LoaderCircle, Plus, X } from "lucide-react";
import { GitHubIcon } from "../shared/GitHubIcon";
import { Tabs } from "../shared/Tabs";
import { postNative } from "../shared/native";
import type { AppModalState } from "./types";

type SourceKind = "local" | "github";
type Source = { kind: SourceKind; value: string; name: string; selected: boolean };

export function CreateProjectForm({ modal, language, onDismiss, onBusyChange }: {
  modal: AppModalState; language: "en" | "es"; onDismiss: () => void; onBusyChange: (busy: boolean) => void;
}) {
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const adding = modal.kind === "add_repository";
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [repositorySetup, setRepositorySetup] = useState<"new" | "existing">(adding ? "existing" : "new");
  const [sourceKind, setSourceKind] = useState<SourceKind>("local");
  const [localSources, setLocalSources] = useState<Source[]>([]);
  const localAddButton = useRef<HTMLButtonElement>(null);
  const [sources, setSources] = useState<Source[]>([]);
  const [mode, setMode] = useState<"link" | "copy">("link");
  const [pending, setPending] = useState<"scan" | "create" | null>(null);
  const [error, setError] = useState("");
  const creatingRepository = !adding && repositorySetup === "new";
  const selected = creatingRepository ? [] : sources.filter(source => source.selected);
  const canSubmit = (adding || !!name.trim()) && (creatingRepository || selected.length > 0);
  const copying = mode === "copy" && selected.some(source => source.kind === "local");
  const downloading = selected.some(source => source.kind === "github");

  useEffect(() => {
    if (!modal.feedback) return;
    setPending(null);
    setError(modal.feedback.error || "");
    const { repositories, completed_sources } = modal.feedback;
    if (repositories) setLocalSources(repositories.map(repo => ({
      kind: "local", value: repo.path, name: repo.name, selected: true,
    })));
    if (completed_sources) setSources(current => current.filter(source => !completed_sources.includes(source.value)));
  }, [modal.feedback]);
  useEffect(() => { onBusyChange(pending !== null); }, [pending, onBusyChange]);
  useEffect(() => {
    if (localSources.length && sourceKind === "local") localAddButton.current?.focus();
  }, [localSources]);

  const chooseLocalFolder = () => {
    if (pending) return;
    setPending("scan"); onBusyChange(true); setError("");
    postNative({ type: "choose_project_modal_folder", request_id: modal.request_id });
  };
  const addLocal = () => {
    if (!localSources.length || pending) return;
    const paths = new Set(localSources.map(source => source.value));
    setSources(current => [
      ...current.map(source => source.kind === "local" && paths.has(source.value) ? { ...source, selected: true } : source),
      ...localSources.filter(source => !current.some(item => item.kind === "local" && item.value === source.value)),
    ]);
    setLocalSources([]); setError("");
  };

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

  const repositoryFields = <div className="project-repository-fields">
    <fieldset className="project-source-picker">
      <legend>{tr("Add repositories from", "Agregar repositorios desde")}</legend>
      <Tabs<SourceKind> value={sourceKind} onValueChange={kind => { setSourceKind(kind); setError(""); }}
        label={tr("Repository source", "Origen del repositorio")} disabled={pending !== null} initialFocus={adding}
        items={[
          { value: "local", label: <><FolderOpen size={16} aria-hidden="true" />{tr("Local folder", "Carpeta local")}</>, content: <>
            <span id="project-local-label" className="project-source-label">{tr("Local folder", "Carpeta local")}</span>
            <div className="project-source-entry">
              <button type="button" className={`project-local-picker${localSources.length ? " has-selection" : ""}`}
                aria-labelledby="project-local-label project-local-selection" aria-describedby="project-local-help"
                title={localSources.length === 1 ? localSources[0].value : undefined} onClick={chooseLocalFolder}>
                <FolderOpen size={16} aria-hidden="true" />
                <span id="project-local-selection">{pending === "scan" ? tr("Searching…", "Buscando…")
                  : localSources.length === 1 ? localSources[0].value
                    : localSources.length ? tr(`${localSources.length} repositories found`, `${localSources.length} repositorios encontrados`)
                      : tr("Choose a folder…", "Elegir una carpeta…")}</span>
              </button>
              <button type="button" className="project-source-add" ref={localAddButton} disabled={!localSources.length} onClick={addLocal}>
                <Plus size={16} aria-hidden="true" />{tr("Add", "Agregar")}
              </button>
            </div>
            <small id="project-local-help">{localSources.length
              ? tr("Click Add to include these repositories, then choose another folder.", "Pulsa Agregar para incluir estos repositorios y luego elige otra carpeta.")
              : tr("Choose a Git repository or a folder containing several to add them to the list.", "Elige un repositorio Git o una carpeta con varios para agregarlos a la lista.")}</small>
          </> },
          { value: "github", label: <><GitHubIcon size={16} />GitHub</>, content: <>
            <label htmlFor="project-github-url">{tr("GitHub repository", "Repositorio de GitHub")}</label>
            <div className="project-source-entry">
              <input id="project-github-url" value={url} onChange={event => setUrl(event.target.value)} placeholder="https://github.com/..." spellCheck={false}
                aria-describedby="project-github-help"
                onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); addGithub(); } }} />
              <button type="button" className="project-source-add" disabled={!url.trim()} onClick={addGithub}>
                <Plus size={16} aria-hidden="true" />{tr("Add", "Agregar")}
              </button>
            </div>
            <small id="project-github-help">{tr("Add one or more repositories. They will be cloned into the project when you confirm.", "Agrega uno o más repositorios. Se clonarán dentro del proyecto al confirmar.")}</small>
          </> },
        ]} />
    </fieldset>
    {sources.length > 0 && <div className="project-repository-heading">
      <span>{tr("Repositories to add", "Repositorios por agregar")} <small>{selected.length}</small></span>
      {sources.length > 1 && <button type="button" className="project-text-button" onClick={() => setSources(current => current.map(source => ({ ...source, selected: selected.length !== sources.length })))}>
        {selected.length === sources.length ? tr("Deselect all", "Quitar selección") : tr("Select all", "Seleccionar todos")}
      </button>}
    </div>}
    {sources.length > 0 ? <div className="project-modal-repositories">
      {sources.map((source, index) => <div className="project-modal-repo" key={source.kind + ":" + source.value}>
        <label className="project-modal-repo-check">
          <input type="checkbox" checked={source.selected} onChange={event => {
            const checked = event.target.checked;
            setSources(current => current.map((item, i) => i === index ? { ...item, selected: checked } : item));
          }} />
          {source.kind === "local" ? <FolderOpen size={16} /> : <GitHubIcon size={16} />}
          <span title={source.value}><strong>{source.name}</strong><small>{source.value}</small></span>
        </label>
        <button type="button" aria-label={tr("Remove ", "Quitar ") + source.name} onClick={() => setSources(current => current.filter((_, i) => i !== index))}><X size={15} /></button>
      </div>)}
    </div> : null}
    {!selected.length && <small className="project-empty-hint">{tr("Add and select at least one repository to continue.", "Agrega y selecciona al menos un repositorio para continuar.")}</small>}
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
  </div>;

  return <form className="create-project-form" onSubmit={event => {
    event.preventDefault();
    if (pending || !canSubmit) return;
    if (!creatingRepository && url.trim()) { setSourceKind("github"); setError(tr("Add the GitHub repository to the list first, or clear its URL.", "Agrega primero el repositorio de GitHub a la lista o borra su URL.")); return; }
    if (!creatingRepository && localSources.length) { setSourceKind("local"); setError(tr("Add the selected local repositories to the list first.", "Agrega primero los repositorios locales seleccionados a la lista.")); return; }
    setError(""); setPending("create"); onBusyChange(true);
    postNative({ type: adding ? "submit_add_repositories" : "submit_create_project", workspace_id: modal.workspace_id, request_id: modal.request_id, name: name.trim(),
      mode, sources: selected.map(({ kind, value }) => ({ kind, value })) });
  }}>
    <p id="app-modal-description">{adding ? modal.name : tr("Your repositories, together in one project.", "Tus repositorios, juntos en un proyecto.")}</p>
    {pending === "create" && <div className="project-import-progress" role="status" aria-live="polite" aria-atomic="true">
      <LoaderCircle className="project-import-progress__spinner" size={36} strokeWidth={1.75} aria-hidden="true" />
      <strong>{copying ? tr("Copying repositories…", "Copiando repositorios…")
        : downloading ? tr("Downloading repositories…", "Descargando repositorios…")
          : adding ? tr("Adding repositories…", "Agregando repositorios…") : tr("Creating your project…", "Creando tu proyecto…")}</strong>
      <p>{copying
        ? tr("Copying files, Git history and dependencies can take several minutes, especially with large repositories.", "Copiar archivos, historial Git y dependencias puede tardar varios minutos, especialmente con repositorios grandes.")
        : downloading ? tr("This may take a few minutes, depending on repository size and your connection.", "Esto puede tardar unos minutos, según el tamaño de los repositorios y tu conexión.")
          : creatingRepository ? tr("Preparing a new Git repository inside your project.", "Preparando un nuevo repositorio Git dentro de tu proyecto.")
            : tr("Preparing your project and its repository links.", "Preparando tu proyecto y los enlaces a sus repositorios.")}</p>
      <small>{tr("Keep Blackholes open until the operation finishes.", "Mantén Blackholes abierto hasta que termine.")}</small>
    </div>}
    <fieldset disabled={pending !== null} hidden={pending === "create"} className="project-modal-fields">
      <legend>{tr("Project settings", "Configuración del proyecto")}</legend>
      {!adding && <label>{tr("Name", "Nombre")}
        <input data-initial-focus required value={name} onChange={event => setName(event.target.value)} placeholder={tr("My project", "Mi proyecto")} />
      </label>}
      {!adding && <fieldset className="project-setup-options">
        <legend>{tr("How do you want to start?", "¿Cómo quieres empezar?")}</legend>
        {(["new", "existing"] as const).map(option => <div
          className={`project-setup-card${repositorySetup === option ? " is-selected" : ""}`} key={option}>
          <label className="project-setup-choice">
            <input type="radio" name="project-repository-setup" value={option} checked={repositorySetup === option}
              onChange={() => { setRepositorySetup(option); setError(""); }} />
            <span><strong>{option === "new" ? tr("Start from scratch", "Empezar desde cero") : tr("Use existing repositories", "Usar repositorios existentes")}</strong>
              <small>{option === "new"
                ? tr("Create a new Git repository inside your project.", "Crea un repositorio Git nuevo dentro de tu proyecto.")
                : tr("Add local folders or clone from GitHub.", "Agrega carpetas locales o clona desde GitHub.")}</small>
            </span>
          </label>
          {option === "existing" && repositorySetup === option && <div className="project-setup-content">{repositoryFields}</div>}
        </div>)}
      </fieldset>}
      {adding && repositoryFields}
      <details className="project-details">
        <summary tabIndex={pending ? -1 : 0} onClick={event => { if (pending) event.preventDefault(); }}>{tr("Location and details", "Ubicación y detalles")}</summary>
        <small>{adding ? tr("Project folder", "Carpeta del proyecto") : tr("Projects folder", "Carpeta de proyectos")}</small>
        <code>{modal.projects_root}</code>
        <p>{tr("Each project has its own instructions. Local links appear directly in its folder and work without Blackholes. GitHub repositories are downloaded there.", "Cada proyecto tiene sus instrucciones. Los enlaces locales aparecen directamente en su carpeta y funcionan sin Blackholes. Los repositorios de GitHub se descargan allí.")}</p>
        {copying && <p>{tr("Copies files, Git history and dependencies. Pause running processes first. Large folders take longer; symbolic links may still point outside the copy.", "Copia archivos, historial Git y dependencias. Pausa los procesos antes de copiar. Las carpetas grandes tardan más; los enlaces simbólicos pueden seguir apuntando fuera de la copia.")}</p>}
      </details>
    </fieldset>
    {error && <p className="app-modal__error" role="alert">{error}</p>}
    <footer>
      <button type="button" className="app-modal__cancel" disabled={pending !== null} onClick={onDismiss}>{modal.cancel_label}</button>
      <button type="submit" className="app-modal__primary" disabled={pending !== null || !canSubmit}>{pending === "create" ? (adding ? tr("Adding…", "Agregando…") : tr("Creating…", "Creando…")) : modal.confirm_label}</button>
    </footer>
  </form>;
}

import { useEffect, useRef, useState } from "react";
import { ChevronRight, FolderGit2, GitBranch, Search, X } from "lucide-react";
import { postNative } from "../shared/native";
import { ModalSelect } from "./ModalSelect";
import type { AppModalState, TaskBranchAvailability } from "./types";

type Preparation = { copy_local_changes: boolean; copy_environment_files: boolean; setup_command: string };
const emptyPreparation = (): Preparation => ({ copy_local_changes: false, copy_environment_files: false, setup_command: "" });
const normalizeRepositoryName = (value: string) => value.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase();

/** Uses the shared modal backdrop while Rust retains all task/worktree operations. */
export function CreateTaskForm({ modal, language, onDismiss, onBusyChange }: {
  modal: AppModalState; language: "en" | "es"; onDismiss: () => void; onBusyChange: (busy: boolean) => void;
}) {
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const [title, setTitle] = useState("");
  const [branch, setBranch] = useState("");
  const [base, setBase] = useState("");
  const [description, setDescription] = useState("");
  const [showDescription, setShowDescription] = useState(false);
  const [branchOptionsOpen, setBranchOptionsOpen] = useState(false);
  const [baseOptionsOpen, setBaseOptionsOpen] = useState(false);
  const repositories = modal.repositories || [];
  const [repositoryQuery, setRepositoryQuery] = useState("");
  const repositorySearch = useRef<HTMLInputElement>(null);
  const repositoryList = useRef<HTMLDivElement>(null);
  const [source, setSource] = useState<"current" | "local" | "remote">("current");
  const [action, setAction] = useState<"reuse" | "recreate">("reuse");
  const [createMissing, setCreateMissing] = useState(false);
  const [replaceDivergent, setReplaceDivergent] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [preparations, setPreparations] = useState<Record<string, Preparation>>({});
  const [pending, setPending] = useState<"check" | "create" | null>(null);
  const [error, setError] = useState("");
  const [branches, setBranches] = useState<TaskBranchAvailability[] | undefined>();
  const searchTerms = normalizeRepositoryName(repositoryQuery).trim().split(/\s+/).filter(Boolean);
  const filtered = searchTerms.length > 0;
  const visibleRepositories = repositories.filter(repository => searchTerms.every(term => normalizeRepositoryName(repository.name).includes(term)));
  const allVisibleSelected = visibleRepositories.length > 0 && visibleRepositories.every(repository => selected.includes(repository.id));
  const hiddenSelectedCount = filtered ? selected.filter(id => !visibleRepositories.some(repository => repository.id === id)).length : 0;
  const usesStartingPoint = source === "current" || createMissing;
  const branchSources = [
    { value: "current" as const, label: tr("New task branch", "Nueva rama de tarea"), description: tr("Start from the current checkout.", "Parte de la rama actual de cada repositorio.") },
    { value: "local" as const, label: tr("Existing local branch", "Rama local existente"), description: tr("Continue a branch on this machine.", "Continúa una rama que ya está en tu equipo.") },
    { value: "remote" as const, label: tr("Branch from origin", "Rama de origin"), description: tr("Continue a remote branch locally.", "Continúa una rama remota en tu equipo.") },
  ];
  const clearBranchFeedback = () => { setBranches(undefined); setError(""); };

  useEffect(() => { if (repositoryList.current) repositoryList.current.scrollTop = 0; }, [repositoryQuery]);

  useEffect(() => {
    if (!modal.feedback) return;
    setPending(null);
    setError(modal.feedback.error || "");
    setBranches(modal.feedback.branches);
  }, [modal.feedback]);
  useEffect(() => { onBusyChange(pending !== null); }, [pending, onBusyChange]);

  const updatePreparation = (id: string, values: Partial<Preparation>) => {
    setPreparations(current => ({ ...current, [id]: { ...(current[id] || emptyPreparation()), ...values } }));
  };
  const send = (checkOnly: boolean) => {
    if (pending || !selected.length || (checkOnly ? !branch.trim() : !title.trim())) return;
    if (source !== "current" && !branch.trim()) {
      setBranchOptionsOpen(true);
      setError(tr("Enter the local or remote branch to use.", "Escribe la rama local o remota que quieres usar."));
      return;
    }
    setError("");
    setPending(checkOnly ? "check" : "create");
    onBusyChange(true);
    postNative({
      type: "create_task_modal", request_id: modal.request_id, workspace_id: modal.workspace_id, check_only: checkOnly,
      request: {
        title: title.trim(), description: description.trim() || null,
        branch_name: branch.trim() || null, base_ref: usesStartingPoint ? base.trim() || null : null,
        branch_source: source, existing_branch_action: action,
        create_missing_branch: createMissing, replace_divergent_local_branches: replaceDivergent,
        repository_ids: selected,
        preparations: Object.fromEntries(selected.map(id => {
          const preparation = preparations[id] || emptyPreparation();
          return [id, { ...preparation, setup_command: preparation.setup_command.trim() || null }];
        })),
      },
    });
  };

  return <form className="create-task-form" onSubmit={event => { event.preventDefault(); send(false); }} onChange={() => { setBranches(undefined); setError(""); }}>
    <p id="app-modal-description">{tr("Work in isolated worktrees for the repositories you choose.", "Trabaja en espacios aislados de los repositorios que elijas.")}</p>
    <fieldset disabled={pending !== null} className="project-modal-fields task-modal-fields">
      <legend>{tr("Task settings", "Configuración de la tarea")}</legend>
      <label>{tr("Title", "Título")}
        <input data-initial-focus required value={title} onChange={event => setTitle(event.target.value)} placeholder={tr("e.g. Fix the login flow", "p. ej. Corregir el login")} />
      </label>
      <button className="project-text-button task-description-toggle" type="button" aria-expanded={showDescription} aria-controls="task-description" onClick={() => setShowDescription(!showDescription)}>
        {showDescription ? tr("Hide description", "Ocultar descripción") : description.trim() ? tr("Edit description", "Editar descripción") : tr("+ Add description", "+ Agregar descripción")}
      </button>
      {showDescription && <label id="task-description">
        <span className="project-visually-hidden">{tr("Description", "Descripción")}</span>
        <textarea rows={2} value={description} onChange={event => setDescription(event.target.value)} placeholder={tr("What needs to be done?", "¿Qué hay que hacer?")} />
      </label>}
      <section className="task-repository-picker" aria-labelledby="task-repositories-title">
        <div className="task-repository-heading">
          <h3 id="task-repositories-title">{tr("Repositories", "Repositorios")}
            <span className={`task-repository-count${selected.length ? " has-selection" : ""}`}>
              <span aria-hidden="true">{selected.length}/{repositories.length}</span>
              <span className="project-visually-hidden">{tr(`${selected.length} of ${repositories.length} selected`, `${selected.length} de ${repositories.length} seleccionados`)}</span>
            </span>
          </h3>
          {repositories.length > 1 && <button type="button" className="project-text-button" disabled={!visibleRepositories.length} onClick={() => {
            const visibleIds = new Set(visibleRepositories.map(repository => repository.id));
            setSelected(current => allVisibleSelected ? current.filter(id => !visibleIds.has(id)) : [...new Set([...current, ...visibleIds])]);
            setBranches(undefined); setError("");
          }}>{allVisibleSelected ? filtered ? tr("Deselect results", "Quitar resultados") : tr("Deselect all", "Quitar selección") : filtered ? tr("Select results", "Seleccionar resultados") : tr("Select all", "Seleccionar todos")}</button>}
        </div>
        <div className="task-repository-browser">
          {!!repositories.length && <div className="task-repository-search">
            <Search size={15} aria-hidden="true" />
            <input ref={repositorySearch} type="search" value={repositoryQuery} aria-label={tr("Search repositories", "Buscar repositorios")}
              aria-controls="task-repository-results" placeholder={tr("Search repositories…", "Buscar repositorios…")} spellCheck={false} autoComplete="off"
              onChange={event => { event.stopPropagation(); setRepositoryQuery(event.target.value); }}
              onKeyDown={event => {
                if (event.key === "Enter") event.preventDefault();
                if (event.key === "Escape" && repositoryQuery) {
                  event.preventDefault(); event.stopPropagation(); setRepositoryQuery("");
                }
              }} />
            {repositoryQuery && <button type="button" className="task-repository-search-clear" aria-label={tr("Clear search", "Limpiar búsqueda")} onClick={() => {
              setRepositoryQuery(""); repositorySearch.current?.focus();
            }}><X size={13} aria-hidden="true" /></button>}
          </div>}
          <div ref={repositoryList} id="task-repository-results" className="task-repository-list">
            {visibleRepositories.map(repository => {
              const checked = selected.includes(repository.id);
              const preparation = preparations[repository.id] || emptyPreparation();
              const configured = [
                preparation.copy_local_changes && tr("changes", "cambios"),
                preparation.copy_environment_files && ".env",
                preparation.setup_command.trim() && tr("command", "comando"),
              ].filter(Boolean).join(" · ");
              return <div key={repository.id} className={`task-modal-repository${checked ? " is-selected" : ""}`}>
                <label className="task-modal-checkbox task-repository-choice"><input type="checkbox" checked={checked} onChange={event => {
                  const value = event.target.checked;
                  setSelected(current => value ? [...current, repository.id] : current.filter(id => id !== repository.id));
                }} /><FolderGit2 size={16} className="task-repository-icon" aria-hidden="true" /><span title={repository.name}>{repository.name}</span></label>
                {checked && <details className="task-repository-options">
                  <summary tabIndex={pending ? -1 : 0} onClick={event => { if (pending) event.preventDefault(); }}>
                    {tr("Preparation", "Preparación")}<span>{configured || tr("Default", "Predeterminada")}</span>
                  </summary>
                  <div className="task-modal-preparation">
                    <label className="task-modal-checkbox"><input type="checkbox" checked={preparation.copy_local_changes} onChange={event => updatePreparation(repository.id, { copy_local_changes: event.target.checked })} />{tr("Copy local changes", "Copiar cambios locales")}</label>
                    <label className="task-modal-checkbox"><input type="checkbox" checked={preparation.copy_environment_files} onChange={event => updatePreparation(repository.id, { copy_environment_files: event.target.checked })} />{tr("Copy .env files", "Copiar archivos .env")}</label>
                    <label>{tr("Setup command", "Comando de preparación")}<input value={preparation.setup_command} onChange={event => updatePreparation(repository.id, { setup_command: event.target.value })} placeholder="npm install" spellCheck={false} />
                      <small>{tr("Optional. Runs when creating this worktree.", "Opcional. Se ejecuta al crear este espacio de trabajo.")}</small>
                    </label>
                  </div>
                </details>}
              </div>;
            })}
            {!visibleRepositories.length && <div className="task-repository-empty" role="status">
              {repositories.length ? <><Search size={22} aria-hidden="true" /><strong>{tr("No repositories found", "No se encontraron repositorios")}</strong><span>{tr("Try another name or clear the search.", "Prueba otro nombre o limpia la búsqueda.")}</span></> : <><FolderGit2 size={22} aria-hidden="true" /><span>{tr("Add repositories to the project before creating a task.", "Agrega repositorios al proyecto antes de crear una tarea.")}</span></>}
            </div>}
          </div>
        </div>
        {!!repositories.length && <div className="task-repository-status" role="status">
          <span>{filtered ? tr(`${visibleRepositories.length} of ${repositories.length} repositories`, `${visibleRepositories.length} de ${repositories.length} repositorios`) : tr("Select the repositories for this task.", "Selecciona los repositorios para esta tarea.")}</span>
          {!!hiddenSelectedCount && <span>{tr(`${hiddenSelectedCount} selected outside this search`, `${hiddenSelectedCount} seleccionados fuera de esta búsqueda`)}</span>}
        </div>}
      </section>
      <details className="task-branch-options" open={branchOptionsOpen} onToggle={event => setBranchOptionsOpen(event.currentTarget.open)}>
        <summary tabIndex={pending ? -1 : 0} onClick={event => { if (pending) event.preventDefault(); }}>
          <ChevronRight size={14} aria-hidden="true" /><strong>{tr("Branch options", "Opciones de rama")}</strong>
          <span title={branch || undefined}>{branch.trim() || (source === "current" ? tr("Automatic name", "Nombre automático") : branchSources.find(option => option.value === source)?.label)}{action === "recreate" && source === "current" ? tr(" · Recreate", " · Recrear") : replaceDivergent && source === "remote" ? tr(" · Replace local", " · Reemplazar local") : ""}</span>
        </summary>
        <div className="task-branch-fields">
          <ModalSelect label={tr("How to start", "Cómo empezar")} value={source} options={branchSources} disabled={pending !== null}
            description={source === "current" ? tr("Starts from the branch currently checked out in each repository.", "Parte de la rama que tienes abierta en cada repositorio.") : source === "local" ? tr("Keeps the local branch’s existing commits.", "Conserva los commits de la rama local.") : tr("Uses the latest commits from origin.", "Usa los últimos commits de origin.")}
            onValueChange={value => { setSource(value); clearBranchFeedback(); }} />
          <label className="task-branch-name">{source === "current" ? tr("Branch name", "Nombre de rama") : source === "local" ? tr("Local branch name", "Nombre de la rama local") : tr("Remote branch name", "Nombre de la rama remota")}
            {source === "current" && <span className="task-branch-optional">{tr("Optional", "Opcional")}</span>}
            <input value={branch} onChange={event => setBranch(event.target.value)} aria-required={source !== "current"}
              placeholder={source === "current" ? tr("Generated from the task title", "Se genera a partir del título") : "feature/existing-work"} spellCheck={false} />
          </label>
          {source === "current" ? <ModalSelect label={tr("If this name already exists", "Si este nombre ya existe")}
            value={action} disabled={pending !== null} onValueChange={value => { setAction(value); clearBranchFeedback(); }}
            options={[
              { value: "reuse", label: tr("Keep existing branch", "Conservar la rama existente"), description: tr("Continue its commits without resetting it.", "Continúa sus commits sin reiniciarla.") },
              { value: "recreate", label: tr("Recreate from starting point", "Recrear desde el punto de partida"), description: tr("Reset the branch and keep a backup of its commits.", "Reinicia la rama y guarda un respaldo de sus commits.") },
            ]} /> : <div className="task-branch-toggles">
            <label className="task-modal-checkbox"><input type="checkbox" checked={createMissing} onChange={event => setCreateMissing(event.target.checked)} /><span>{tr("Create it if missing", "Crearla si no existe")}<small>{tr("Starts from the current checkout or the starting point below.", "Parte de la rama actual o de la base que elijas abajo.")}</small></span></label>
            {source === "remote" && <label className="task-modal-checkbox"><input type="checkbox" checked={replaceDivergent} onChange={event => setReplaceDivergent(event.target.checked)} /><span>{tr("Match the remote if local commits differ", "Usar la versión remota si la local difiere")}<small>{tr("Keeps a backup of the local commits before replacing them.", "Guarda un respaldo de los commits locales antes de reemplazarlos.")}</small></span></label>}
          </div>}
          {source === "current" && action === "recreate" && <p className="task-branch-warning">{tr("The existing branch will start over. Its current commits are kept in a backup branch.", "La rama existente volverá a empezar. Sus commits actuales se guardan en una rama de respaldo.")}</p>}
          {usesStartingPoint && <details className="task-branch-base" open={baseOptionsOpen} onToggle={event => setBaseOptionsOpen(event.currentTarget.open)}>
            <summary tabIndex={pending ? -1 : 0} onClick={event => { if (pending) event.preventDefault(); }}>
              <ChevronRight size={13} aria-hidden="true" /><strong>{tr("Starting point", "Punto de partida")}</strong><span title={base || undefined}>{base.trim() || tr("Current checkout", "Rama actual")}</span>
            </summary>
            <label>{tr("Branch or commit", "Rama o commit")}<span className="task-branch-optional">{tr("Optional", "Opcional")}</span>
              <input value={base} onChange={event => setBase(event.target.value)} placeholder="main" spellCheck={false} />
              <small>{tr("Used when creating or recreating a branch. Leave empty to start from each repository’s current checkout.", "Se usa al crear o recrear una rama. Déjalo vacío para partir de la rama actual de cada repositorio.")}</small>
            </label>
          </details>}
          {!!branch.trim() && <div className="task-branch-check">
            <button className="task-modal-check" type="button" disabled={!selected.length || !branch.trim()} onClick={() => send(true)}>
              <GitBranch size={14} aria-hidden="true" />{pending === "check" ? tr("Checking…", "Comprobando…") : tr("Check availability", "Comprobar disponibilidad")}
            </button>
            <small>{!selected.length ? tr("Select a repository first.", "Selecciona un repositorio primero.") : tr("Checks the selected repositories.", "Comprueba los repositorios seleccionados.")}</small>
          </div>}
          {branches && <div className="task-modal-availability" aria-live="polite">{branches.map(result => {
            const exists = source === "remote" ? Boolean(result.remoteRevision) : Boolean(result.localRevision);
            const state = exists ? tr("branch exists", "la rama existe") : source === "current" ? tr("new branch", "rama nueva") : tr("branch missing", "rama no encontrada");
            return <small key={result.repositoryId}>{result.repositoryName}: {state}
              {result.localCheckedOut && tr(" · already checked out", " · ya está en uso")}
              {!exists && result.base && " · " + tr("from", "desde") + " " + result.base.label}
            </small>;
          })}</div>}
        </div>
      </details>
    </fieldset>
    {error && <p className="app-modal__error" role="alert">{error}</p>}
    <footer>
      <button type="button" className="app-modal__cancel" disabled={pending !== null} onClick={onDismiss}>{tr("Cancel", "Cancelar")}</button>
      <button type="submit" className="app-modal__primary" disabled={pending !== null || !title.trim() || !selected.length}>
        {pending === "create" ? tr("Preparing worktrees…", "Preparando espacios…") : tr("Create task", "Crear tarea")}
      </button>
    </footer>
  </form>;
}

import { useEffect, useState } from "react";
import { postNative } from "../shared/native";
import type { AppModalState, TaskBranchAvailability } from "./types";

type Preparation = { copy_local_changes: boolean; copy_environment_files: boolean; setup_command: string };
const emptyPreparation = (): Preparation => ({ copy_local_changes: false, copy_environment_files: false, setup_command: "" });

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
  const repositories = modal.repositories || [];
  const [source, setSource] = useState<"current" | "local" | "remote">("current");
  const [action, setAction] = useState<"reuse" | "recreate">("reuse");
  const [createMissing, setCreateMissing] = useState(false);
  const [replaceDivergent, setReplaceDivergent] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [preparations, setPreparations] = useState<Record<string, Preparation>>({});
  const [pending, setPending] = useState<"check" | "create" | null>(null);
  const [error, setError] = useState("");
  const [branches, setBranches] = useState<TaskBranchAvailability[] | undefined>();

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
        branch_name: branch.trim() || null, base_ref: base.trim() || null,
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
      <div className="project-repository-heading">
        <span>{tr("Repositories", "Repositorios")} <small>{selected.length}/{repositories.length}</small></span>
        {repositories.length > 1 && <button type="button" className="project-text-button" onClick={() => {
          setSelected(selected.length === repositories.length ? [] : repositories.map(repository => repository.id)); setBranches(undefined); setError("");
        }}>{selected.length === repositories.length ? tr("Deselect all", "Quitar selección") : tr("Select all", "Seleccionar todos")}</button>}
      </div>
      <div className="task-repository-list">
        {repositories.map(repository => {
          const checked = selected.includes(repository.id);
          const preparation = preparations[repository.id] || emptyPreparation();
          const configured = [
            preparation.copy_local_changes && tr("changes", "cambios"),
            preparation.copy_environment_files && ".env",
            preparation.setup_command.trim() && tr("command", "comando"),
          ].filter(Boolean).join(" · ");
          return <div key={repository.id} className="task-modal-repository">
            <label className="task-modal-checkbox"><input type="checkbox" checked={checked} onChange={event => {
              const value = event.target.checked;
              setSelected(current => value ? [...current, repository.id] : current.filter(id => id !== repository.id));
            }} /><span>{repository.name}</span></label>
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
        {!repositories.length && <small>{tr("Add repositories to the project before creating a task.", "Agrega repositorios al proyecto antes de crear una tarea.")}</small>}
      </div>
      {!!repositories.length && !selected.length && <small className="task-selection-hint">{tr("Choose at least one repository.", "Elige al menos un repositorio.")}</small>}
      <details className="task-branch-options" open={branchOptionsOpen} onToggle={event => setBranchOptionsOpen(event.currentTarget.open)}>
        <summary tabIndex={pending ? -1 : 0} onClick={event => { if (pending) event.preventDefault(); }}>
          {tr("Branch options", "Opciones de rama")}
          <span title={branch || undefined}>{branch.trim() || tr("Automatic name", "Nombre automático")}{action === "recreate" && source === "current" ? tr(" · Recreate", " · Recrear") : replaceDivergent && source === "remote" ? tr(" · Replace divergent", " · Reemplazar divergentes") : ""}</span>
        </summary>
        <div className="task-branch-fields">
          <label>{tr("Branch name", "Nombre de rama")}
            <input value={branch} onChange={event => setBranch(event.target.value)} placeholder="feature/new-branch" spellCheck={false} />
            {source === "current" && <small>{tr("Leave empty to use the task title.", "Si lo dejas vacío, se usa el título de la tarea.")}</small>}
          </label>
          <label className="task-select-row" htmlFor="task-branch-source">{tr("Start from", "Partir de")}
            <select id="task-branch-source" value={source} onChange={event => setSource(event.target.value as typeof source)}>
              <option value="current">{tr("Current state (HEAD)", "Estado actual (HEAD)")}</option>
              <option value="local">{tr("Local branch", "Rama local")}</option>
              <option value="remote">{tr("Remote branch", "Rama remota")}</option>
            </select>
          </label>
          <label>{tr("Base branch · optional", "Rama base · opcional")}
            <input value={base} onChange={event => setBase(event.target.value)} placeholder={tr("Current HEAD by default", "HEAD actual por defecto")} spellCheck={false} />
          </label>
          {source === "current" ? <label className="task-select-row" htmlFor="task-existing-branch">{tr("If the branch exists", "Si la rama ya existe")}
            <select id="task-existing-branch" value={action} onChange={event => setAction(event.target.value as typeof action)}>
              <option value="reuse">{tr("Reuse it", "Reutilizarla")}</option>
              <option value="recreate">{tr("Recreate it", "Recrearla")}</option>
            </select>
          </label> : <>
            <label className="task-modal-checkbox"><input type="checkbox" checked={createMissing} onChange={event => setCreateMissing(event.target.checked)} />{tr("Create the branch if missing", "Crear la rama si no existe")}</label>
            {source === "remote" && <label className="task-modal-checkbox"><input type="checkbox" checked={replaceDivergent} onChange={event => setReplaceDivergent(event.target.checked)} />{tr("Replace divergent local branches (keeps a backup)", "Reemplazar ramas locales divergentes (con respaldo)")}</label>}
          </>}
          {source === "current" && action === "recreate" && <p className="task-branch-warning">{tr("Recreates an existing branch from the chosen base or current HEAD. Use reuse to keep its current starting point.", "Recrea una rama existente desde la base elegida o el HEAD actual. Usa reutilizar para conservar su punto de partida.")}</p>}
          <button className="task-modal-check" type="button" disabled={!selected.length || !branch.trim()} onClick={() => send(true)}>
            {pending === "check" ? tr("Checking…", "Comprobando…") : tr("Check branch", "Comprobar rama")}
          </button>
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

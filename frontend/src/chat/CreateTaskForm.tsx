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

  return <form onSubmit={event => { event.preventDefault(); send(false); }} onChange={() => { setBranches(undefined); setError(""); }}>
    <p id="app-modal-description" className="sr-only">{tr("Configure an isolated task and its repositories.", "Configura una tarea aislada y sus repositorios.")}</p>
    <fieldset disabled={pending !== null} className="project-modal-fields task-modal-fields">
      <legend>{tr("Task settings", "Configuración de la tarea")}</legend>
      <label>{tr("Task title", "Título de la tarea")}
        <input data-initial-focus required value={title} onChange={event => setTitle(event.target.value)} placeholder={tr("e.g. Fix the login flow", "p. ej. Corregir el login")} />
      </label>
      <label>{tr("Branch name (optional)", "Nombre de rama (opcional)")}
        <input value={branch} onChange={event => setBranch(event.target.value)} placeholder="feature/new-branch" spellCheck={false} />
      </label>
      <label>{tr("Description (optional)", "Descripción (opcional)")}
        <textarea rows={3} value={description} onChange={event => setDescription(event.target.value)} placeholder={tr("Short context for this task…", "Contexto breve para esta tarea…")} />
      </label>
      <div className="task-modal-section">
        <strong>{tr("Branch source", "Origen de la rama")}</strong>
        <div className="project-modal-modes">
          {([
            ["current", tr("Current HEAD", "HEAD actual")], ["local", tr("Local branch", "Rama local")], ["remote", tr("Remote branch", "Rama remota")],
          ] as const).map(([value, label]) => <button key={value} type="button" aria-pressed={source === value} onClick={() => { setSource(value); setBranches(undefined); }}>
            {label}
          </button>)}
        </div>
        <label>{tr("Base branch (optional)", "Rama base (opcional)")}
          <input value={base} onChange={event => setBase(event.target.value)} placeholder={tr("Empty uses the current HEAD", "Vacío usa el HEAD actual")} spellCheck={false} />
        </label>
        {source === "current" ? <div className="project-modal-modes">
          {([
            ["reuse", tr("Reuse existing branch", "Reutilizar rama existente")], ["recreate", tr("Recreate from current HEAD", "Recrear desde el HEAD actual")],
          ] as const).map(([value, label]) => <button key={value} type="button" aria-pressed={action === value} onClick={() => setAction(value)}>{label}</button>)}
        </div> : <>
          <label className="task-modal-checkbox"><input type="checkbox" checked={createMissing} onChange={event => setCreateMissing(event.target.checked)} />{tr("Create the branch when it is missing", "Crear la rama cuando no exista")}</label>
          {source === "remote" && <label className="task-modal-checkbox"><input type="checkbox" checked={replaceDivergent} onChange={event => setReplaceDivergent(event.target.checked)} />{tr("Replace divergent local branches (a backup is kept)", "Reemplazar ramas locales divergentes (se conserva un respaldo)")}</label>}
        </>}
        <button className="task-modal-check" type="button" disabled={!selected.length || !branch.trim()} onClick={() => send(true)}>
          {pending === "check" ? tr("Checking…", "Comprobando…") : tr("Check branch", "Comprobar rama")}
        </button>
        {branches && <div className="task-modal-availability" aria-live="polite">{branches.map(result => {
          const exists = source === "remote" ? Boolean(result.remoteRevision) : Boolean(result.localRevision);
          const state = exists ? tr("branch exists", "la rama existe") : source === "current" ? tr("new branch", "rama nueva") : tr("branch missing", "rama no encontrada");
          return <small key={result.repositoryId}>{result.repositoryName}: {state}
            {result.localCheckedOut && tr(" · already checked out", " · ya está en uso")}
            {!exists && result.base && ` · ${tr("from", "desde")} ${result.base.label}`}
          </small>;
        })}</div>}
      </div>
      <div className="task-modal-section">
        <strong>{tr("Repositories", "Repositorios")}</strong>
        {(modal.repositories || []).map(repository => {
          const checked = selected.includes(repository.id);
          const preparation = preparations[repository.id] || emptyPreparation();
          return <div key={repository.id} className="task-modal-repository">
            <label className="task-modal-checkbox"><input type="checkbox" checked={checked} onChange={event => {
              setSelected(current => event.target.checked ? [...current, repository.id] : current.filter(id => id !== repository.id));
            }} />{repository.name}</label>
            {checked && <div className="task-modal-preparation">
              <label className="task-modal-checkbox"><input type="checkbox" checked={preparation.copy_local_changes} onChange={event => updatePreparation(repository.id, { copy_local_changes: event.target.checked })} />{tr("Copy current local changes", "Copiar cambios locales actuales")}</label>
              <label className="task-modal-checkbox"><input type="checkbox" checked={preparation.copy_environment_files} onChange={event => updatePreparation(repository.id, { copy_environment_files: event.target.checked })} />{tr("Copy .env files", "Copiar archivos .env")}</label>
              <label>{tr("Setup command (optional)", "Comando de preparación (opcional)")}<input value={preparation.setup_command} onChange={event => updatePreparation(repository.id, { setup_command: event.target.value })} placeholder="e.g. npm install" spellCheck={false} /></label>
            </div>}
          </div>;
        })}
      </div>
    </fieldset>
    {error && <p className="app-modal__error" role="alert">{error}</p>}
    <footer>
      <button type="button" className="app-modal__cancel" disabled={pending !== null} onClick={onDismiss}>{modal.cancel_label}</button>
      <button type="submit" className="app-modal__primary" disabled={pending !== null || !title.trim() || !selected.length}>
        {pending === "create" ? tr("Preparing worktrees…", "Preparando worktrees…") : modal.confirm_label}
      </button>
    </footer>
  </form>;
}

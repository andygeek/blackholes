import { useEffect, useState } from "react";
import { ChevronDown, Code2, Database, Folder, GitBranch, Globe2, Layers3, ListTodo, Rocket, SquareTerminal, type LucideIcon } from "lucide-react";
import { postNative } from "../shared/native";
import type { AppModalState } from "./types";

const icons: Record<string, LucideIcon> = {
  layers: Layers3, folder: Folder, "code-2": Code2, "square-terminal": SquareTerminal,
  rocket: Rocket, database: Database, globe: Globe2, "list-todo": ListTodo, "git-branch": GitBranch,
};

export function EditProjectForm({ modal, language, onDismiss, onBusyChange }: {
  modal: AppModalState; language: "en" | "es"; onDismiss(): void; onBusyChange(busy: boolean): void;
}) {
  const tr = (en: string, es: string) => language === "es" ? es : en;
  const [name, setName] = useState(modal.name);
  const [icon, setIcon] = useState(modal.icon || "layers");
  const [color, setColor] = useState(modal.color_id || "slate");
  const [showIcons, setShowIcons] = useState(false);
  const [pending, setPending] = useState(false);
  const SelectedIcon = icons[icon] || Layers3;
  const accent = modal.color_options?.find(option => option.value === color)?.color;
  const iconLabel = modal.icon_options?.find(option => option.value === icon)?.label || tr("Layers", "Capas");

  useEffect(() => {
    if (modal.feedback) { setPending(false); onBusyChange(false); }
  }, [modal.feedback, onBusyChange]);

  return <form className="edit-project-form" onSubmit={event => {
    event.preventDefault();
    if (pending || !name.trim()) return;
    setPending(true); onBusyChange(true);
    postNative({ type: "submit_edit_project", request_id: modal.request_id, workspace_id: modal.workspace_id,
      name: name.trim(), icon, color });
  }}>
    <p id="app-modal-description">{modal.description}</p>
    <fieldset className="project-modal-fields" disabled={pending}>
      <legend>{tr("Project appearance", "Apariencia del proyecto")}</legend>
      <label>{tr("Visible name", "Nombre visible")}
        <input data-initial-focus required value={name} onChange={event => setName(event.target.value)} />
      </label>
      <div className="edit-project-choice" role="group" aria-labelledby="edit-project-icon-label">
        <span id="edit-project-icon-label">{tr("Project icon", "Icono del proyecto")}</span>
        <button type="button" className="edit-project-icon-trigger" aria-expanded={showIcons}
          aria-controls="edit-project-icons" onClick={() => setShowIcons(value => !value)}>
          <SelectedIcon size={20} style={{ color: accent }} aria-hidden="true" />
          <span>{iconLabel}</span><ChevronDown size={16} aria-hidden="true" />
        </button>
        <div id="edit-project-icons" className="note-icon-options" hidden={!showIcons}>
          {modal.icon_options?.map(option => {
            const OptionIcon = icons[option.value] || Layers3;
            return <button key={option.value} type="button" aria-label={option.label} title={option.label}
              aria-pressed={icon === option.value} style={{ color: accent }}
              onClick={() => setIcon(option.value)}><OptionIcon size={24} aria-hidden="true" /></button>;
          })}
        </div>
      </div>
      <div className="edit-project-choice" role="group" aria-labelledby="edit-project-color-label">
        <span id="edit-project-color-label">{tr("Project color", "Color del proyecto")}</span>
        <div className="edit-project-colors">
          {modal.color_options?.map(option => <button key={option.value} type="button"
            aria-label={option.label} title={option.label} aria-pressed={color === option.value}
            style={{ background: option.color }} onClick={() => setColor(option.value)} />)}
        </div>
      </div>
    </fieldset>
    {modal.feedback?.error && <p className="app-modal__error" role="alert">{modal.feedback.error}</p>}
    <footer>
      <button type="button" className="app-modal__cancel" disabled={pending} onClick={onDismiss}>{modal.cancel_label}</button>
      <button type="submit" className="app-modal__confirm app-modal__primary" disabled={pending || !name.trim()}>
        {pending ? tr("Saving…", "Guardando…") : modal.confirm_label}
      </button>
    </footer>
  </form>;
}

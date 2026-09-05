import {
  ArrowLeft,
  Bot,
  Cable,
  ChevronDown,
  ChevronRight,
  Code2,
  Database,
  ExternalLink,
  File,
  FileCode2,
  Folder,
  FolderOpen,
  GitBranch,
  Globe2,
  Layers3,
  ListTodo,
  NotebookPen,
  Plus,
  Puzzle,
  RefreshCw,
  Rocket,
  Save,
  Search,
  Settings,
  ShieldCheck,
  SquareTerminal,
  Trash2,
  Wrench,
  X,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { postNative } from "../shared/native";
import { SidebarResizeHandle } from "../shared/SidebarResizeHandle";
import { MonacoSurface } from "./MonacoSurface";
import { RepositoryExplorer } from "./RepositoryExplorer";
import { applyAppTheme, type AppTheme } from "../shared/theme";
import {
  NotionNoteEditor,
  type NoteBlocks,
  type NoteDocumentChange,
} from "./NotionNoteEditor";

export type SurfaceKind = "settings" | "project-settings" | "note" | "workbench" | "unassigned-agent";

interface UnassignedAgentData {
  title: string;
  description: string;
}

export interface WorkspaceSurfaceEvent {
  type: "workspace_surface";
  surface: SurfaceKind;
  theme: AppTheme;
  data: SettingsData | ProjectSettingsData | NoteData | WorkbenchData | UnassignedAgentData;
}

interface Choice {
  disabled?: boolean;
  value: string;
  label: string;
  icon?: LucideIcon;
}

interface AuthenticationState {
  status: "connecting" | "needs-input" | "connected" | "error";
  detail: string;
  opened_url?: string | null;
}

interface SkillItem {
  name: string;
  description: string;
  path: string;
  enabled: boolean;
}

interface McpItem {
  name: string;
  source: string;
  enabled: boolean;
  required: boolean;
  managed?: boolean;
  transport?: string | null;
  authentication_status?: "needs-auth" | "connecting" | "connected" | "error" | null;
  authentication_detail?: string | null;
}

interface UsageCard {
  label: string;
  value: string;
  detail: string;
  utilization?: number | null;
}

export interface SettingsData {
  language: "en" | "es";
  theme: AppTheme;
  projects_root: string;
  git_available: boolean;
  provider: string;
  provider_label: string;
  auth_mode: string;
  authentication?: AuthenticationState | null;
  model: string;
  model_options: Choice[];
  model_catalog_loading?: boolean;
  model_catalog_error?: boolean;
  model_control_supported: boolean;
  effort?: string | null;
  effort_options: Choice[];
  full_access: boolean;
  permission_control_supported: boolean;
  skills: SkillItem[];
  mcps: McpItem[];
  external_mcp_control_supported: boolean;
  usage_cards: UsageCard[];
  token_detail: string;
  usage_updated: string;
  sidebar_width: number;
  usage_refreshing: boolean;
  usage_refresh_error: boolean;
}

export interface ProjectSettingsData {
  language: "en" | "es";
  theme: AppTheme;
  workspace_id: string;
  title: string;
  skills: SkillItem[];
  mcps: McpItem[];
  external_mcp_control_supported: boolean;
  project_instructions: string;
  project_revision: number;
  task_instructions: string;
  task_revision: number;
  error?: string | null;
}

export interface NoteData {
  language: "en" | "es";
  theme: AppTheme;
  owner: "project" | "task";
  id: string;
  document_id: string;
  title: string;
  icon: string;
  color: string;
  color_id: string;
  content: string;
  blocks?: NoteBlocks | null;
  preview: boolean;
  save_state: "saved" | "saving" | "error";
  revision: number;
  icon_options: Choice[];
  color_options: Array<Choice & { color: string }>;
}

export interface ExplorerRow {
  path: string;
  label: string;
  depth: number;
  hidden: boolean;
  expanded: boolean;
  selected: boolean;
  kind: "directory" | "file" | "symlink" | "loading" | "error";
}

export interface ChangeRow {
  relative_path: string;
  previous_relative_path?: string | null;
  kind: "added" | "deleted" | "modified" | "renamed" | "untracked" | "conflicted";
  selected: boolean;
}

export interface ExplorerData {
  open: boolean;
  root_label: string;
  root_path: string;
  mode: "files" | "changes";
  rows: ExplorerRow[];
  changes: ChangeRow[];
  changes_state: "idle" | "loading" | "ready" | "error";
  changes_error?: string | null;
}

interface EditorData {
  request_id: number;
  state: "loading" | "ready" | "error";
  error?: string | null;
  file_name: string;
  relative_path: string;
  content: string;
  language: string;
  source: "repository" | "project-instructions" | "task-instructions";
  workspace_id?: string | null;
  save_state: "saved" | "saving" | "error";
  revision: number;
}

interface DiffRow {
  row_type: "hunk" | "line";
  kind?: "context" | "changed" | "added" | "deleted";
  old_number?: number | null;
  new_number?: number | null;
  old_text?: string;
  new_text?: string;
  old_start?: number;
  new_start?: number;
  header?: string;
}

interface DiffData {
  original?: string | null;
  modified?: string | null;
  request_id: number;
  state: "loading" | "ready" | "error" | "binary" | "empty";
  error?: string | null;
  file_name: string;
  relative_path: string;
  change_kind: ChangeRow["kind"];
  rows: DiffRow[];
  truncated: boolean;
}

export interface WorkbenchData {
  language: "en" | "es";
  theme: AppTheme;
  explorer: ExplorerData;
  editor?: EditorData | null;
  diff?: DiffData | null;
}

const t = (language: "en" | "es", english: string, spanish: string) => language === "en" ? english : spanish;

const readStoredNumber = (key: string, fallback: number): number => {
  try {
    const value = Number(window.localStorage.getItem(key));
    return Number.isFinite(value) ? value : fallback;
  } catch {
    return fallback;
  }
};

const storeNumber = (key: string, value: number): void => {
  try {
    window.localStorage.setItem(key, String(value));
  } catch {
    // Embedded WebViews may use an opaque origin where WebKit disables storage.
  }
};

const iconFor = (value?: string): LucideIcon => ({
  layers: Layers3,
  folder: Folder,
  "code-2": Code2,
  "square-terminal": SquareTerminal,
  rocket: Rocket,
  database: Database,
  globe: Globe2,
  "list-todo": ListTodo,
  "git-branch": GitBranch,
}[value || ""] || NotebookPen);

const saveStateLabel = (state: NoteData["save_state"], language: "en" | "es") => ({
  saved: t(language, "Saved", "Guardado"),
  saving: t(language, "Saving…", "Guardando…"),
  error: t(language, "Could not save", "No se pudo guardar"),
})[state];

function ChoiceGroup({ choices, value, onChange }: {
  choices: Choice[];
  value?: string | null;
  onChange(value: string): void;
}) {
  return (
    <div className="workspace-choice-group">
      {choices.map((choice) => {
        const ChoiceIcon = choice.icon;
        return (
          <button
            key={choice.value}
            type="button"
            className={choice.value === value ? "is-selected" : ""}
            onClick={() => onChange(choice.value)}
          >
            {ChoiceIcon && <ChoiceIcon size={14} />}
            {choice.label}
          </button>
        );
      })}
    </div>
  );
}

function SettingsSection({ title, description, children, wide = false }: {
  title: string;
  description?: string;
  children: React.ReactNode;
  wide?: boolean;
}) {
  return (
    <section className={`settings-section${wide ? " is-wide" : ""}`}>
      <h2>{title}</h2>
      {description && <p>{description}</p>}
      {children}
    </section>
  );
}

interface SettingsTab<T extends string> {
  value: T;
  label: string;
  icon: LucideIcon;
  badge?: number;
}

function SettingsTabs<T extends string>({ tabs, value, onChange, label }: {
  tabs: SettingsTab<T>[];
  value: T;
  onChange(value: T): void;
  label: string;
}) {
  return (
    <nav className="settings-tabs" aria-label={label}>
      {tabs.map((tab) => {
        const Icon = tab.icon;
        return (
          <button
            key={tab.value}
            type="button"
            className={value === tab.value ? "is-selected" : ""}
            aria-current={value === tab.value ? "page" : undefined}
            onClick={() => onChange(tab.value)}
          >
            <Icon size={15} />
            <span>{tab.label}</span>
            {typeof tab.badge === "number" && <b>{tab.badge}</b>}
          </button>
        );
      })}
    </nav>
  );
}

function McpList({
  items,
  language,
  onToggle,
  onConnect,
  onCancelConnection,
  onRemove,
}: {
  items: McpItem[];
  language: "en" | "es";
  onToggle(item: McpItem): void;
  onConnect?(item: McpItem): void;
  onCancelConnection?(item: McpItem): void;
  onRemove?(item: McpItem): void;
}) {
  const [confirmingRemoval, setConfirmingRemoval] = useState<string | null>(null);
  return (
    <div className="skills-list mcp-list">
      {items.map((mcp) => (
        <article key={mcp.name}>
          <span><Cable size={16} /></span>
          <div>
            <strong>{mcp.name}</strong>
            <p>{mcp.source}</p>
            {mcp.required && <small>{t(language, "Required by Blackholes", "Requerido por Blackholes")}</small>}
            {mcp.authentication_status && (
              <small className={`mcp-auth-status is-${mcp.authentication_status}`}>
                {mcp.authentication_status === "connected"
                  ? t(language, "Connected", "Conectado")
                  : mcp.authentication_status === "connecting"
                    ? t(language, "Waiting for browser authorization…", "Esperando autorización en el navegador…")
                    : mcp.authentication_status === "error"
                      ? t(language, "Connection failed", "Falló la conexión")
                      : t(language, "Not connected", "Sin conectar")}
                {mcp.authentication_detail ? ` · ${mcp.authentication_detail}` : ""}
              </small>
            )}
          </div>
          <div className="mcp-item-actions">
            {mcp.authentication_status && onConnect && (
              <button
                type="button"
                className="workspace-button mcp-connect-button"
                onClick={() => mcp.authentication_status === "connecting"
                  ? onCancelConnection?.(mcp)
                  : onConnect(mcp)}
              >
                {mcp.authentication_status === "connecting"
                  ? t(language, "Cancel", "Cancelar")
                  : mcp.authentication_status === "connected"
                    ? t(language, "Reconnect", "Reconectar")
                    : mcp.authentication_status === "error"
                      ? t(language, "Retry", "Reintentar")
                      : t(language, "Connect", "Conectar")}
              </button>
            )}
            <button
              type="button"
              className={`workspace-switch${mcp.enabled ? " is-enabled" : ""}`}
              aria-label={mcp.name}
              aria-pressed={mcp.enabled}
              disabled={mcp.required}
              onClick={() => onToggle(mcp)}
            ><i /></button>
            {mcp.managed && onRemove && (confirmingRemoval === mcp.name ? (
              <div className="mcp-remove-confirmation">
                <button type="button" onClick={() => setConfirmingRemoval(null)}>{t(language, "Cancel", "Cancelar")}</button>
                <button type="button" className="is-danger" onClick={() => {
                  setConfirmingRemoval(null);
                  onRemove(mcp);
                }}>{t(language, "Remove", "Eliminar")}</button>
              </div>
            ) : (
              <button
                type="button"
                className="mcp-remove-button"
                title={t(language, "Remove from project", "Eliminar del proyecto")}
                aria-label={t(language, `Remove ${mcp.name}`, `Eliminar ${mcp.name}`)}
                onClick={() => setConfirmingRemoval(mcp.name)}
              ><Trash2 size={14} /></button>
            ))}
          </div>
        </article>
      ))}
    </div>
  );
}

type McpTransport = "http" | "stdio";

interface McpDraft {
  name: string;
  transport: McpTransport;
  url: string;
  oauthClientId: string;
  oauthCallbackPort: string;
  command: string;
  args: string;
  env: string;
}

const mcpPreset = (preset: "slack" | "clickup" | "custom"): McpDraft => {
  if (preset === "slack") return {
    name: "slack",
    transport: "http",
    url: "https://mcp.slack.com/mcp",
    oauthClientId: "",
    oauthCallbackPort: "3118",
    command: "",
    args: "",
    env: "",
  };
  if (preset === "clickup") return {
    name: "clickup",
    transport: "http",
    url: "https://mcp.clickup.com/mcp",
    oauthClientId: "",
    oauthCallbackPort: "",
    command: "",
    args: "",
    env: "",
  };
  return {
    name: "",
    transport: "http",
    url: "",
    oauthClientId: "",
    oauthCallbackPort: "",
    command: "",
    args: "",
    env: "",
  };
};

function ProjectMcpInstaller({ workspaceId, language }: {
  workspaceId: string;
  language: "en" | "es";
}) {
  const [open, setOpen] = useState(false);
  const [preset, setPreset] = useState<"slack" | "clickup" | "custom">("clickup");
  const [draft, setDraft] = useState<McpDraft>(() => mcpPreset("clickup"));
  const [error, setError] = useState("");
  const selectPreset = (next: "slack" | "clickup" | "custom") => {
    setPreset(next);
    setDraft(mcpPreset(next));
    setError("");
  };
  const update = (values: Partial<McpDraft>) => setDraft((current) => ({ ...current, ...values }));

  if (!open) return (
    <button className="workspace-button" type="button" onClick={() => setOpen(true)}>
      <Plus size={14} /> {t(language, "Install MCP", "Instalar MCP")}
    </button>
  );

  return (
    <form className="mcp-installer" onSubmit={(event) => {
      event.preventDefault();
      if (preset === "slack" && !draft.oauthClientId.trim()) {
        setError(t(language, "Enter the Client ID of your approved Slack app.", "Ingresa el Client ID de tu app de Slack aprobada."));
        return;
      }
      const env: Record<string, string> = {};
      for (const line of draft.env.split("\n").map((value) => value.trim()).filter(Boolean)) {
        const separator = line.indexOf("=");
        if (separator <= 0) {
          setError(t(language, "Environment variables must use KEY=VALUE, one per line.", "Las variables de entorno deben usar CLAVE=VALOR, una por línea."));
          return;
        }
        env[line.slice(0, separator).trim()] = line.slice(separator + 1);
      }
      const callbackPort = Number.parseInt(draft.oauthCallbackPort, 10);
      postNative({
        type: "install_project_agent_mcp",
        workspace_id: workspaceId,
        name: draft.name.trim(),
        transport: draft.transport,
        url: draft.transport === "http" ? draft.url.trim() : null,
        oauth_client_id: draft.transport === "http" ? draft.oauthClientId.trim() || null : null,
        oauth_callback_port: draft.transport === "http" && Number.isFinite(callbackPort) ? callbackPort : null,
        command: draft.transport === "stdio" ? draft.command.trim() : null,
        args: draft.transport === "stdio" ? draft.args.split("\n").map((value) => value.trim()).filter(Boolean) : [],
        env: draft.transport === "stdio" ? env : {},
      });
      setOpen(false);
      setError("");
    }}>
      <header>
        <div>
          <strong>{t(language, "Install an MCP for this project", "Instalar un MCP para este proyecto")}</strong>
          <p>{t(language, "It will be available to project agents and their isolated tasks.", "Estará disponible para los agentes del proyecto y sus tareas aisladas.")}</p>
        </div>
        <button type="button" className="workspace-icon-button" aria-label={t(language, "Close", "Cerrar")} onClick={() => { setOpen(false); setError(""); }}><X size={15} /></button>
      </header>
      <div className="workspace-choice-group mcp-presets">
        {(["clickup", "slack", "custom"] as const).map((value) => (
          <button key={value} type="button" className={preset === value ? "is-selected" : ""} onClick={() => selectPreset(value)}>
            {value === "custom" ? t(language, "Custom", "Personalizado") : value === "clickup" ? "ClickUp" : "Slack"}
          </button>
        ))}
      </div>
      <div className="mcp-form-grid">
        <label>
          <span>{t(language, "Server name", "Nombre del servidor")}</span>
          <input value={draft.name} onChange={(event) => update({ name: event.target.value })} placeholder="my-mcp" required />
        </label>
        <label>
          <span>{t(language, "Transport", "Transporte")}</span>
          <select value={draft.transport} onChange={(event) => update({ transport: event.target.value as McpTransport })}>
            <option value="http">HTTP</option>
            <option value="stdio">{t(language, "Local command (stdio)", "Comando local (stdio)")}</option>
          </select>
        </label>
        {draft.transport === "http" ? (
          <>
            <label className="is-wide">
              <span>URL</span>
              <input type="url" value={draft.url} onChange={(event) => update({ url: event.target.value })} placeholder="https://example.com/mcp" required />
            </label>
            <label>
              <span>{t(language, "OAuth client ID (optional)", "Client ID OAuth (opcional)")}</span>
              <input value={draft.oauthClientId} onChange={(event) => update({ oauthClientId: event.target.value })} />
            </label>
            <label>
              <span>{t(language, "OAuth callback port (optional)", "Puerto callback OAuth (opcional)")}</span>
              <input type="number" min="1" max="65535" value={draft.oauthCallbackPort} onChange={(event) => update({ oauthCallbackPort: event.target.value })} />
            </label>
          </>
        ) : (
          <>
            <label className="is-wide">
              <span>{t(language, "Command", "Comando")}</span>
              <input value={draft.command} onChange={(event) => update({ command: event.target.value })} placeholder="npx" required />
            </label>
            <label>
              <span>{t(language, "Arguments · one per line", "Argumentos · uno por línea")}</span>
              <textarea value={draft.args} onChange={(event) => update({ args: event.target.value })} placeholder={"-y\npackage-name"} />
            </label>
            <label>
              <span>{t(language, "Environment · KEY=VALUE", "Entorno · CLAVE=VALOR")}</span>
              <textarea value={draft.env} onChange={(event) => update({ env: event.target.value })} placeholder="API_KEY=…" />
            </label>
          </>
        )}
      </div>
      {preset === "slack" && (
        <p className="mcp-installer__hint">{t(language, "Slack requires an approved Slack app and its OAuth client ID. After installation, use Connect on its card.", "Slack requiere una app de Slack aprobada y su Client ID OAuth. Después de instalarlo, usa Conectar en su tarjeta.")}</p>
      )}
      {preset === "clickup" && (
        <p className="mcp-installer__hint">{t(language, "ClickUp uses its official remote MCP. After installation, use Connect on its card to authorize it in your browser.", "ClickUp usa su MCP remoto oficial. Después de instalarlo, usa Conectar en su tarjeta para autorizarlo en el navegador.")}</p>
      )}
      {error && <p className="mcp-installer__error">{error}</p>}
      <footer>
        <button type="button" className="workspace-button" onClick={() => { setOpen(false); setError(""); }}>{t(language, "Cancel", "Cancelar")}</button>
        <button type="submit" className="workspace-button is-primary">{t(language, "Install for project", "Instalar en el proyecto")}</button>
      </footer>
    </form>
  );
}

type PreferencePage = "general" | "accounts" | "models" | "permissions" | "usage" | "skills" | "mcps";

function PreferenceGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return <section className="preference-group"><h2>{title}</h2><div className="preference-box">{children}</div></section>;
}

function PreferenceRow({ title, description, children }: {
  title: string;
  description?: React.ReactNode;
  children: React.ReactNode;
}) {
  return <div className="preference-row">
    <div className="preference-row__copy"><h3>{title}</h3>{description && <div className="preference-row__description">{description}</div>}</div>
    <div className="preference-row__control">{children}</div>
  </div>;
}

function PreferenceSelect({ label, choices, value, onChange }: {
  label: string; choices: Choice[]; value?: string | null; onChange(value: string): void;
}) {
  const selected = value ?? choices.find((choice) => choice.value === "automatic")?.value ?? "";
  return <span className="preference-select">
    <select aria-label={label} value={selected} onChange={(event) => onChange(event.target.value)}>
      {!choices.some((choice) => choice.value === selected) && <option value={selected}>{selected || "—"}</option>}
      {choices.map((choice) => <option key={choice.value} value={choice.value} disabled={choice.disabled}>{choice.label}</option>)}
    </select>
    <ChevronDown size={14} aria-hidden="true" />
  </span>;
}

function SettingsView({ data }: { data: SettingsData }) {
  const language = data.language;
  const [authCode, setAuthCode] = useState("");
  const [activePage, setActivePage] = useState<PreferencePage>("general");
  const [query, setQuery] = useState("");
  const contentRef = useRef<HTMLDivElement>(null);
  useEffect(() => { if (contentRef.current) contentRef.current.scrollTop = 0; }, [activePage, query]);
  useEffect(() => setAuthCode(""), [data.provider, data.auth_mode]);
  useEffect(() => {
    if (activePage === "usage") postNative({ type: "refresh_plan_usage" });
    if (activePage === "models") postNative({ type: "refresh_model_catalog" });
  }, [activePage, data.provider, data.auth_mode]);
  const providers: Choice[] = [
    { value: "claude", label: "Claude" }, { value: "codex", label: "Codex" },
    { value: "gemini", label: "Gemini" }, { value: "opencode", label: "OpenCode · Generic" },
  ];
  const providerControl = <PreferenceSelect label={t(language, "Agent provider", "Proveedor del agente")}
    choices={providers} value={data.provider} onChange={(provider) => postNative({ type: "set_agent_provider", provider })} />;
  const providerRow = <PreferenceRow title={t(language, "Agent provider", "Proveedor del agente")}
    description={t(language, "Choose the active runtime for upcoming responses.", "Elige el motor activo para las próximas respuestas.")}>{providerControl}</PreferenceRow>;

  const pages: Array<{ id: PreferencePage; label: string; description: string; icon: LucideIcon; keywords: string; badge?: number; content: React.ReactNode }> = [
    {
      id: "general", label: t(language, "General", "General"), icon: Settings,
      description: t(language, "Make Blackholes feel at home.", "Personaliza la apariencia y el espacio de trabajo."),
      keywords: "appearance apariencia theme tema claro oscuro light dark language idioma English Español projects proyectos carpeta folder " + data.projects_root,
      content: <>
        {!data.git_available && <PreferenceGroup title={t(language, "Finish setup", "Completar instalación")}>
          <PreferenceRow title={t(language, "Git tools", "Herramientas de Git")} description={t(language,
            "Node and the agent runtimes are included in the app. Cloning repositories also requires Apple's Command Line Tools. Install them, then check again.",
            "Node y los motores de agentes vienen incluidos en la app. Para clonar repositorios también necesitas las herramientas de Apple. Instálalas y vuelve a comprobar.")}>
            <div className="inline-actions">
              <button className="workspace-button" type="button" onClick={() => postNative({ type: "install_git_tools" })}>{t(language, "Install Git tools", "Instalar herramientas de Git")}</button>
              <button className="workspace-button" type="button" onClick={() => postNative({ type: "refresh_runtime_status" })}>{t(language, "Check again", "Comprobar de nuevo")}</button>
            </div>
          </PreferenceRow>
        </PreferenceGroup>}
        <PreferenceGroup title={t(language, "Preferences", "Preferencias")}>
          <PreferenceRow title={t(language, "Appearance", "Apariencia")} description={t(language, "Theme for the entire app, including terminals.", "Tema de toda la aplicación, incluidas las terminales.")}>
            <PreferenceSelect label={t(language, "Appearance", "Apariencia")} choices={[{ value: "light", label: t(language, "Light", "Claro") }, { value: "dark", label: t(language, "Dark", "Oscuro") }]} value={data.theme} onChange={(theme) => { applyAppTheme(theme); postNative({ type: "set_theme", theme }); }} />
          </PreferenceRow>
          <PreferenceRow title={t(language, "Language", "Idioma")} description={t(language, "Language for the app interface.", "Idioma de la interfaz.")}>
            <PreferenceSelect label={t(language, "Language", "Idioma")} choices={[{ value: "en", label: "English" }, { value: "es", label: "Español" }]} value={language} onChange={(value) => postNative({ type: "set_language", language: value })} />
          </PreferenceRow>
        </PreferenceGroup>
        <PreferenceGroup title={t(language, "Workspace", "Espacio de trabajo")}>
          <PreferenceRow title={t(language, "Projects folder", "Carpeta de proyectos")} description={<>
            <span>{t(language, "Each project gets its own folder with cloned repositories, skills, and instructions. Existing project locations are preserved.", "Cada proyecto tiene su carpeta con repositorios clonados, skills e instrucciones. Las ubicaciones de proyectos existentes se conservan.")}</span>
            <code className="preference-path">{data.projects_root}</code>
          </>}>
            <div className="inline-actions">
              <button className="workspace-button" type="button" onClick={() => postNative({ type: "reveal_projects_root" })}><FolderOpen size={14} />{t(language, "Show", "Mostrar")}</button>
              <button className="workspace-button" type="button" onClick={() => postNative({ type: "choose_projects_root" })}>{t(language, "Change…", "Cambiar…")}</button>
            </div>
          </PreferenceRow>
        </PreferenceGroup>
      </>,
    },
    {
      id: "accounts", label: t(language, "Accounts", "Cuentas"), icon: Bot,
      description: t(language, "Choose an agent provider and connect your account.", "Elige un proveedor de agentes y conecta tu cuenta."),
      keywords: "runtime motor proveedor provider Claude Codex Gemini OpenCode auth authentication autenticar conexión cuenta account",
      content: <>
        <PreferenceGroup title={t(language, "Provider and account", "Proveedor y cuenta")}>
          {providerRow}
          <PreferenceRow title={t(language, "Account source", "Origen de la cuenta")} description={t(language, "Use your computer profile or a separate Blackholes profile.", "Usa el perfil de tu computadora o uno separado para Blackholes.")}>
            <PreferenceSelect label={t(language, "Account source", "Origen de la cuenta")} choices={[
              { value: "system", label: t(language, "Computer", "Computadora") },
              { value: "isolated", label: "Blackholes" },
            ]} value={data.auth_mode} onChange={(auth_mode) => postNative({ type: "set_agent_auth_mode", auth_mode })} />
          </PreferenceRow>
          <PreferenceRow title={t(language, "Authentication", "Autenticación")} description={t(language, "Connect or change the account for ", "Conecta o cambia la cuenta de ") + data.provider_label + "."}>
            <button className="workspace-button" type="button" onClick={() => postNative({ type: "authenticate_agent_provider" })}>{t(language, "Connect / change account", "Conectar / cambiar cuenta")}</button>
          </PreferenceRow>
        </PreferenceGroup>
        {data.authentication && (
                <div className={`auth-card is-${data.authentication.status}`}>
                  <strong>{({
                    connecting: t(language, "Connecting account…", "Conectando cuenta…"),
                    "needs-input": t(language, "Authorization required", "Autorización requerida"),
                    connected: t(language, "Account connected", "Cuenta conectada"),
                    error: t(language, "Could not connect", "No se pudo conectar"),
                  })[data.authentication.status]}</strong>
                  <p>{data.authentication.detail}</p>
                  {data.authentication.status === "needs-input" && (
                    <form onSubmit={(event) => {
                      event.preventDefault();
                      if (!authCode.trim()) return;
                      postNative({ type: "submit_agent_auth", value: authCode.trim() });
                      setAuthCode("");
                    }}>
                      <input value={authCode} onChange={(event) => setAuthCode(event.target.value)} autoFocus aria-label={t(language, "Authorization code", "Código de autorización")} placeholder={t(language, "Authorization code", "Código de autorización")} />
                      <button className="workspace-button" type="submit">{t(language, "Continue", "Continuar")}</button>
                    </form>
                  )}
                  <div className="inline-actions">
                    {data.authentication.opened_url && data.authentication.status !== "connected" && (
                      <button className="workspace-button" type="button" onClick={() => postNative({ type: "open_url", url: data.authentication?.opened_url })}>
                        <ExternalLink size={13} /> {t(language, "Open browser again", "Abrir navegador de nuevo")}
                      </button>
                    )}
                    <button className="workspace-button" type="button" onClick={() => postNative({ type: "cancel_agent_auth" })}>
                      {t(language, "Close", "Cerrar")}
                    </button>
                  </div>
                </div>
              )}
      </>,
    },
    {
      id: "models", label: t(language, "Models", "Modelos"), icon: Bot,
      description: t(language, "Model and reasoning defaults for upcoming responses.", "Modelo y razonamiento para las próximas respuestas."),
      keywords: "reasoning razonamiento esfuerzo effort modelo model automatic automático " + data.model_options.map((option) => option.label).join(" "),
      content: <PreferenceGroup title={t(language, "Response settings", "Ajustes de respuesta")}>
        <PreferenceRow title={t(language, "Account", "Cuenta")} description={data.provider_label}>
          <button className="workspace-button" type="button" onClick={() => setActivePage("accounts")}>{t(language, "Manage account", "Administrar cuenta")}</button>
        </PreferenceRow>
        <PreferenceRow title={t(language, "Provider catalog", "Catálogo del proveedor")} description={data.model_catalog_loading
          ? t(language, "Loading models for this account…", "Cargando modelos de esta cuenta…")
          : data.model_catalog_error
            ? t(language, "Could not load models. Check your account and installed runtime, then retry.", "No se pudieron cargar los modelos. Revisa la cuenta y el motor instalado, y vuelve a intentar.")
            : t(language, "Reported by the active provider and account. Access is ultimately validated by the provider.", "Reportado por el proveedor y la cuenta activa. El proveedor valida el acceso al usarlo.")}>
          <button type="button" className="workspace-button" disabled={data.model_catalog_loading} onClick={() => postNative({ type: "refresh_model_catalog", force: true })}>
            <RefreshCw size={14} />{t(language, "Refresh", "Actualizar")}
          </button>
        </PreferenceRow>
        <PreferenceRow title={t(language, "Model", "Modelo")} description={data.provider_label}>
          {data.model_control_supported
            ? <PreferenceSelect label={t(language, "Model", "Modelo")} choices={data.model_options} value={data.model} onChange={(model) => postNative({ type: "set_agent_model", model })} />
            : <span className="preference-value">{data.model_options.find((option) => option.value === data.model)?.label || data.model || t(language, "Managed by provider", "Gestionado por el proveedor")}</span>}
        </PreferenceRow>
        {data.effort_options.length > 0 && <PreferenceRow title={t(language, "Reasoning effort", "Esfuerzo de razonamiento")} description={t(language, "How much effort the model spends on its response.", "Cuánto esfuerzo dedica el modelo a preparar la respuesta.")}>
          <PreferenceSelect label={t(language, "Reasoning effort", "Esfuerzo de razonamiento")} choices={data.effort_options} value={data.effort} onChange={(effort) => postNative({ type: "set_agent_effort", effort })} />
        </PreferenceRow>}
        {data.effort_options.length === 0 && <p className="preference-footnote" style={{ paddingBottom: 16 }}>{t(language, "Reasoning is managed by the provider; no configurable levels were reported for this selection.", "El proveedor gestiona el razonamiento; no reportó niveles configurables para esta selección.")}</p>}
      </PreferenceGroup>,
    },
    {
      id: "permissions", label: t(language, "Permissions", "Permisos"), icon: ShieldCheck,
      description: t(language, "Default access for agents across the app.", "Acceso predeterminado de los agentes en toda la aplicación."),
      keywords: "full access acceso total standard estándar permissions permisos seguridad security sandbox",
      content: <PreferenceGroup title={t(language, "Agent access", "Acceso de agentes")}>
        <PreferenceRow title={t(language, "Permission mode", "Modo de permisos")} description={t(language, "Shared setting for runtimes that support permission modes. Applies with or without isolated tasks.", "Ajuste compartido por los motores que admiten modos de permisos. Se aplica con o sin tareas aisladas.")}>
          <PreferenceSelect label={t(language, "Permission mode", "Modo de permisos")} choices={[
            { value: "full", label: t(language, "Full access", "Acceso total") }, { value: "standard", label: t(language, "Standard", "Estándar") },
          ]} value={data.full_access ? "full" : "standard"} onChange={(value) => postNative({ type: "set_agents_full_access", enabled: value === "full" })} />
        </PreferenceRow>
      </PreferenceGroup>,
    },
    {
      id: "usage", label: t(language, "Usage", "Consumo"), icon: Database,
      description: t(language, "Reported plan limits and accumulated agent usage.", "Límites reportados del plan y consumo acumulado de los agentes."),
      keywords: "billing facturación costo cost usage consumo tokens límites limits plan balance weekly semanal hours horas",
      content: <>
        <div className="preference-toolbar">
          <span>{data.provider_label} · {data.auth_mode === "isolated" ? "Blackholes" : t(language, "Computer account", "Cuenta de la computadora")}</span>
          <button type="button" className="workspace-button" disabled={data.usage_refreshing} onClick={() => postNative({ type: "refresh_plan_usage" })}>
            <RefreshCw size={14} />{data.usage_refreshing ? t(language, "Updating…", "Actualizando…") : t(language, "Refresh limits", "Actualizar límites")}
          </button>
        </div>
        <p className="preference-footnote" role="status">{data.usage_refresh_error
          ? t(language, "Could not refresh limits. Showing the last report; you can retry.", "No se pudieron actualizar los límites. Se muestra el último reporte; puedes reintentar.")
          : t(language, "Limits belong to the selected account. Cost and tokens are local totals for this provider, not your subscription bill.", "Los límites corresponden a la cuenta seleccionada. El costo y los tokens son totales locales de este proveedor, no la factura de tu suscripción.")}</p>
        <PreferenceGroup title={t(language, "Plan and limits", "Plan y límites")}>
          {data.usage_cards.length === 0 && <p className="preference-empty">{t(language, "No usage reported yet.", "Todavía no hay consumo reportado.")}</p>}
          {data.usage_cards.map((card, index) => <PreferenceRow key={`${index}-${card.label}`} title={card.label} description={card.detail}>
            <div className="preference-usage">
              <strong>{card.value}</strong>
              {typeof card.utilization === "number" && Number.isFinite(card.utilization) && <progress max={100} value={Math.min(100, Math.max(0, card.utilization))} aria-label={card.label} aria-valuetext={card.value + ". " + card.detail} />}
            </div>
          </PreferenceRow>)}
        </PreferenceGroup>
        <div className="preference-footnote">{data.token_detail && <p>{data.token_detail}</p>}{data.usage_updated && <p>{data.usage_updated}</p>}</div>
      </>,
    },
    {
      id: "skills", label: "Skills", icon: Wrench, badge: data.skills.filter((skill) => skill.enabled).length,
      description: t(language, "Manage the skills available to Black Bots.", "Administra las skills disponibles para los Black Bots."),
      keywords: "skills habilidades capacidades import importar " + data.skills.map((skill) => skill.name + " " + skill.description).join(" "),
      content: <>
        <div className="preference-toolbar">
          <span>{data.skills.filter((skill) => skill.enabled).length + " / " + data.skills.length + t(language, " enabled", " activas")}</span>
          <div className="inline-actions">
            <button className="workspace-button" type="button" onClick={() => postNative({ type: "reveal_agent_skills" })}><FolderOpen size={14} />{t(language, "Show folder", "Mostrar carpeta")}</button>
            <button className="workspace-button" type="button" onClick={() => postNative({ type: "import_agent_skills" })}><Plus size={14} />{t(language, "Import skills", "Importar skills")}</button>
          </div>
        </div>
        <div className="skills-list preference-integrations">
          {data.skills.length === 0 && <p className="preference-empty">{t(language, "No skills imported yet. Import a skill folder to get started.", "Todavía no importaste skills. Importa una carpeta de skill para empezar.")}</p>}
          {data.skills.map((skill) => <article key={skill.name}>
            <span><Wrench size={16} /></span>
            <div><strong>{skill.name}</strong><details className="preference-item-details"><summary>{t(language, "Description and location", "Descripción y ubicación")}</summary><p>{skill.description}</p><code>{skill.path}</code></details></div>
            <button type="button" role="switch" className={"workspace-switch" + (skill.enabled ? " is-enabled" : "")}
              aria-label={skill.name} aria-checked={skill.enabled}
              onClick={() => postNative({ type: "set_agent_skill_enabled", name: skill.name, enabled: !skill.enabled })}><i /></button>
          </article>)}
        </div>
        <p className="preference-footnote">{t(language, "Only imported and enabled skills are available. You can also choose skills per project.", "Solo estarán disponibles las skills importadas y activadas. También puedes elegirlas por proyecto.")}</p>
      </>,
    },
    {
      id: "mcps", label: t(language, "MCP servers", "Servidores MCP"), icon: Cable, badge: data.mcps.filter((mcp) => mcp.enabled).length,
      description: t(language, "Connections and tools available to your agents.", "Conexiones y herramientas disponibles para tus agentes."),
      keywords: "mcp servers servidores conexiones integrations integraciones " + data.mcps.map((mcp) => mcp.name + " " + mcp.source).join(" "),
      content: <>
        <div className="preference-toolbar"><span>{data.provider_label}</span><button className="workspace-button" type="button" onClick={() => setActivePage("accounts")}>{t(language, "Manage account", "Administrar cuenta")}</button></div>
        <p className="preference-footnote">{data.external_mcp_control_supported
          ? t(language, "Blackholes is built in and always enabled. Manage project-specific installations in project settings.", "Blackholes está integrado y siempre activo. Administra las instalaciones específicas en la configuración del proyecto.")
          : t(language, "This adapter currently exposes only the built-in Blackholes MCP.", "Este adaptador actualmente expone solo el MCP integrado de Blackholes.")}</p>
        <McpList items={data.mcps} language={language} onToggle={(mcp) => postNative({ type: "set_agent_mcp_enabled", name: mcp.name, enabled: !mcp.enabled })} />
      </>,
    },
  ];
  const normalizeSearch = (value: string) => value.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase();
  const terms = normalizeSearch(query).trim().split(/\s+/).filter(Boolean);
  const visiblePages = terms.length ? pages.filter((page) => {
    const text = normalizeSearch(page.label + " " + page.description + " " + page.keywords);
    return terms.every((term) => text.includes(term));
  }) : pages.filter((page) => page.id === activePage);
  const active = pages.find((page) => page.id === activePage)!;

  return <main className="workspace-page settings-page preferences-layout" style={{ gridTemplateColumns: `${data.sidebar_width}px minmax(0, 1fr)` }}>
    <SidebarResizeHandle width={data.sidebar_width} left={data.sidebar_width - 6} edge="center" label={t(language, "Resize sidebar", "Cambiar ancho del menú lateral")} />
    <aside className="preferences-sidebar">
      <div className="preferences-sidebar__scroll">
      <button type="button" className="preferences-back" onClick={() => postNative({ type: "close_settings" })}><ArrowLeft size={16} />{t(language, "Back to app", "Volver a la app")}</button>
      <h1>{t(language, "Settings", "Configuración")}</h1>
      <div className="preferences-search">
        <Search size={15} aria-hidden="true" />
        <input type="search" aria-label={t(language, "Search settings", "Buscar ajustes")} placeholder={t(language, "Search settings…", "Buscar ajustes…")} value={query} onChange={(event) => setQuery(event.target.value)} />
      </div>
      <nav aria-label={t(language, "Settings sections", "Secciones de configuración")}>
        {pages.map((page, index) => <div key={page.id}>
          {(index === 0 || index === 1 || index === 5) && <span className="preferences-nav-label">{index === 0 ? t(language, "Application", "Aplicación") : index === 1 ? t(language, "Agents", "Agentes") : t(language, "Integrations", "Integraciones")}</span>}
          <button type="button" className={!terms.length && activePage === page.id ? "is-selected" : ""}
            aria-current={!terms.length && activePage === page.id ? "page" : undefined}
            onClick={() => { setActivePage(page.id); setQuery(""); }}>
            <page.icon size={16} /><span>{page.label}</span>{page.badge !== undefined && <small>{page.badge}</small>}
          </button>
        </div>)}
      </nav>
      <p className="preferences-sidebar__hint">{t(language, "Changes are saved automatically.", "Los cambios se guardan automáticamente.")}</p>
      </div>
    </aside>
    <div className="preferences-main" ref={contentRef}>
      <div className="preferences-content">
        <header className="preferences-heading">
          <h1>{terms.length ? t(language, "Search results", "Resultados de búsqueda") : active.label}</h1>
          <p>{terms.length ? t(language, "Settings matching your search.", "Ajustes que coinciden con tu búsqueda.") : active.description}</p>
        </header>
        {visiblePages.length === 0 && <div className="preference-empty"><Search size={24} /><p>{t(language, "No matching settings. Try “model”, “language”, or “MCP”.", "No hay coincidencias. Prueba con «modelo», «idioma» o «MCP».")}</p></div>}
        {visiblePages.map((page) => <section className="preferences-page-content" key={page.id} aria-label={page.label}>
          {terms.length > 0 && <h2 className="preference-result-title">{page.label}</h2>}
          {page.content}
        </section>)}
      </div>
    </div>
  </main>;
}

type ProjectInstructionKind = "project" | "tasks";

function ProjectInstructionsEditor({
  workspaceId,
  kind,
  content,
  revision,
  language,
}: {
  workspaceId: string;
  kind: ProjectInstructionKind;
  content: string;
  revision: number;
  language: "en" | "es";
}) {
  const [value, setValue] = useState(content);
  const [saveState, setSaveState] = useState<"saved" | "saving">("saved");
  const timer = useRef<number | null>(null);
  const pending = useRef<string | null>(null);

  const sendPending = () => {
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = null;
    const nextContent = pending.current;
    if (nextContent === null) return;
    pending.current = null;
    postNative({
      type: kind === "project" ? "update_project_instructions" : "update_project_task_instructions",
      workspace_id: workspaceId,
      content: nextContent,
    });
    setSaveState("saved");
  };

  useEffect(() => {
    if (pending.current !== null) return;
    setValue(content);
    setSaveState("saved");
  }, [content, revision, workspaceId, kind]);

  useEffect(() => () => {
    if (timer.current) window.clearTimeout(timer.current);
    const nextContent = pending.current;
    if (nextContent === null) return;
    postNative({
      type: kind === "project" ? "update_project_instructions" : "update_project_task_instructions",
      workspace_id: workspaceId,
      content: nextContent,
    });
    pending.current = null;
  }, [kind, workspaceId]);

  return (
    <div className="project-instructions-editor">
      <header>
        <code>{kind === "project" ? "CLAUDE.md" : ".blackholes-task-CLAUDE.md"}</code>
        <span className={`save-state is-${saveState}`}>{saveStateLabel(saveState, language)}</span>
      </header>
      <textarea
        value={value}
        spellCheck={false}
        onChange={(event) => {
          const nextContent = event.currentTarget.value;
          setValue(nextContent);
          pending.current = nextContent;
          setSaveState("saving");
          if (timer.current) window.clearTimeout(timer.current);
          timer.current = window.setTimeout(sendPending, 650);
        }}
        onBlur={sendPending}
      />
    </div>
  );
}

function ProjectSettingsView({ data }: { data: ProjectSettingsData }) {
  const language = data.language;
  const [activeTab, setActiveTab] = useState<"capabilities" | "instructions">("capabilities");
  const enabledCapabilities = data.skills.filter((skill) => skill.enabled).length
    + data.mcps.filter((mcp) => mcp.enabled).length;
  const tabs: SettingsTab<"capabilities" | "instructions">[] = [
    { value: "capabilities", label: t(language, "Capabilities", "Capacidades"), icon: Puzzle, badge: enabledCapabilities },
    { value: "instructions", label: t(language, "Agent instructions", "Instrucciones de agentes"), icon: FileCode2 },
  ];
  return (
    <main className="workspace-page settings-page project-settings-page">
      <div className="workspace-page__content">
        <header className="workspace-title">
          <span><Settings size={20} /></span>
          <div>
            <h1>{t(language, `Settings for ${data.title}`, `Configuración de ${data.title}`)}</h1>
            <p>{t(language, "Keep agent context and skills isolated for this project.", "Mantén aislados el contexto y las skills de los agentes de este proyecto.")}</p>
          </div>
        </header>

        {data.error && <div className="workspace-inline-error">{data.error}</div>}

        <SettingsTabs
          tabs={tabs}
          value={activeTab}
          onChange={(value) => setActiveTab(value as typeof activeTab)}
          label={t(language, "Project settings sections", "Secciones de configuración del proyecto")}
        />

        {activeTab === "capabilities" && (
          <div className="settings-tab-panel">
        <SettingsSection
          wide
          title={`${t(language, "PROJECT SKILLS", "SKILLS DEL PROYECTO")} · ${data.skills.filter((skill) => skill.enabled).length}/${data.skills.length}`}
          description={t(
            language,
            "Choose which globally enabled skills are available to agents working in this project and its tasks.",
            "Elige cuáles de las skills habilitadas globalmente podrán usar los agentes de este proyecto y sus tareas.",
          )}
        >
          <div className="skills-list">
            {data.skills.length === 0 && <p>{t(language, "Enable or import skills from the general settings first.", "Primero importa o habilita skills desde la configuración general.")}</p>}
            {data.skills.map((skill) => (
              <article key={skill.name}>
                <span><Wrench size={16} /></span>
                <div><strong>{skill.name}</strong><p>{skill.description}</p><code>{skill.path}</code></div>
                <button
                  type="button"
                  className={`workspace-switch${skill.enabled ? " is-enabled" : ""}`}
                  aria-label={skill.name}
                  aria-pressed={skill.enabled}
                  onClick={() => postNative({
                    type: "set_project_agent_skill_enabled",
                    workspace_id: data.workspace_id,
                    name: skill.name,
                    enabled: !skill.enabled,
                  })}
                ><i /></button>
              </article>
            ))}
          </div>
        </SettingsSection>

        <SettingsSection
          wide
          title={`${t(language, "PROJECT MCP SERVERS", "SERVIDORES MCP DEL PROYECTO")} · ${data.mcps.filter((mcp) => mcp.enabled).length}/${data.mcps.length}`}
          description={data.external_mcp_control_supported
            ? t(language, "Install MCPs for this project and choose which ones its agents and isolated tasks may use.", "Instala MCPs para este proyecto y elige cuáles podrán usar sus agentes y tareas aisladas.")
            : t(language, "This adapter currently supports only the required Blackholes MCP.", "Este adaptador actualmente admite solo el MCP obligatorio de Blackholes.")}
        >
          {data.external_mcp_control_supported && (
            <div className="inline-actions">
              <ProjectMcpInstaller workspaceId={data.workspace_id} language={language} />
            </div>
          )}
          <McpList
            items={data.mcps}
            language={language}
            onToggle={(mcp) => postNative({
              type: "set_project_agent_mcp_enabled",
              workspace_id: data.workspace_id,
              name: mcp.name,
              enabled: !mcp.enabled,
            })}
            onConnect={(mcp) => postNative({
              type: "authenticate_project_agent_mcp",
              workspace_id: data.workspace_id,
              name: mcp.name,
            })}
            onCancelConnection={(mcp) => postNative({
              type: "cancel_project_agent_mcp_authentication",
              workspace_id: data.workspace_id,
              name: mcp.name,
            })}
            onRemove={(mcp) => postNative({
              type: "remove_project_agent_mcp",
              workspace_id: data.workspace_id,
              name: mcp.name,
            })}
          />
        </SettingsSection>
          </div>
        )}

        {activeTab === "instructions" && (
          <div className="settings-tab-panel">
        <SettingsSection
          wide
          title={t(language, "PROJECT CLAUDE.MD", "CLAUDE.MD DEL PROYECTO")}
          description={t(
            language,
            "Instructions and context for agents working directly in the project.",
            "Instrucciones y contexto para los agentes que trabajan directamente en el proyecto.",
          )}
        >
          <ProjectInstructionsEditor
            workspaceId={data.workspace_id}
            kind="project"
            content={data.project_instructions}
            revision={data.project_revision}
            language={language}
          />
        </SettingsSection>

        <SettingsSection
          wide
          title={t(language, "CLAUDE.MD FOR TASKS", "CLAUDE.MD PARA TAREAS")}
          description={t(
            language,
            "This shared template is added to every new task and reapplied to existing tasks whenever it is saved.",
            "Esta plantilla compartida se agrega a cada tarea nueva y se vuelve a aplicar a las tareas existentes cada vez que se guarda.",
          )}
        >
          <ProjectInstructionsEditor
            workspaceId={data.workspace_id}
            kind="tasks"
            content={data.task_instructions}
            revision={data.task_revision}
            language={language}
          />
        </SettingsSection>
          </div>
        )}
      </div>
    </main>
  );
}

function NoteView({ data }: { data: NoteData }) {
  const NoteIcon = iconFor(data.icon);
  const timer = useRef<number | null>(null);
  const pending = useRef<NoteDocumentChange | null>(null);
  const [saveState, setSaveState] = useState(data.save_state);

  useEffect(() => {
    if (pending.current && data.save_state === "saved") return;
    setSaveState(data.save_state);
  }, [data.revision, data.save_state]);

  useEffect(() => {
    pending.current = null;
    if (timer.current) window.clearTimeout(timer.current);
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
      const change = pending.current;
      if (!change) return;
      postNative({
        type: "update_note",
        owner: data.owner,
        id: data.id,
        content: change.content,
        blocks: change.blocks,
      });
      pending.current = null;
    };
  }, [data.document_id, data.id, data.owner]);

  const queueSave = (change: NoteDocumentChange) => {
    pending.current = change;
    setSaveState("saving");
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      const current = pending.current;
      if (!current) return;
      postNative({
        type: "update_note",
        owner: data.owner,
        id: data.id,
        content: current.content,
        blocks: current.blocks,
      });
      pending.current = null;
    }, 650);
  };

  const flushPending = () => {
    if (timer.current) window.clearTimeout(timer.current);
    const current = pending.current;
    if (!current) return;
    postNative({
      type: "update_note",
      owner: data.owner,
      id: data.id,
      content: current.content,
      blocks: current.blocks,
    });
    pending.current = null;
  };

  return (
    <main className="workspace-page note-page notion-note-page">
      <header className="note-toolbar">
        <span className={`save-state is-${saveState}`}>{saveStateLabel(saveState, data.language)}</span>
        <button className="workspace-icon-button" type="button" title={t(data.language, "Reload", "Recargar")} onClick={() => postNative({ type: "reload_note", owner: data.owner, id: data.id })}><RefreshCw size={15} /></button>
      </header>
      <article className="note-document">
        <details className="appearance-picker">
          <summary style={{ color: data.color }}><NoteIcon size={34} /></summary>
          <div>
            <span>{t(data.language, "Icon", "Icono")}</span>
            <ChoiceGroup choices={data.icon_options} value={data.icon} onChange={(icon) => postNative({ type: "set_note_appearance", owner: data.owner, id: data.id, icon })} />
            <span>{t(data.language, "Color", "Color")}</span>
            <div className="color-options">{data.color_options.map((option) => <button key={option.value} type="button" className={option.value === data.color_id ? "is-selected" : ""} style={{ background: option.color }} aria-label={option.label} onClick={() => postNative({ type: "set_note_appearance", owner: data.owner, id: data.id, color: option.value })} />)}</div>
          </div>
        </details>
        <h1>{data.title}</h1>
        <NotionNoteEditor
          documentId={data.document_id}
          language={data.language}
          markdown={data.content}
          blocks={data.blocks}
          theme={data.theme}
          onChange={queueSave}
          onBlur={flushPending}
        />
      </article>
    </main>
  );
}

const changeMarker: Record<ChangeRow["kind"], string> = { added: "A", deleted: "D", modified: "M", renamed: "R", untracked: "U", conflicted: "!" };


function FileEditor({ editor, language, theme, root }: { editor: EditorData; language: "en" | "es"; theme: AppTheme; root: string }) {
  return (
    <section className="file-editor-shell">
      {editor.source !== "repository" && editor.workspace_id && (
        <nav className="instructions-tabs">
          <button className={editor.source === "project-instructions" ? "is-selected" : ""} onClick={() => postNative({ type: "open_project_instructions", workspace_id: editor.workspace_id })}>{t(language, "Project CLAUDE.md", "CLAUDE.md del proyecto")}</button>
          <button className={editor.source === "task-instructions" ? "is-selected" : ""} onClick={() => postNative({ type: "open_project_task_instructions", workspace_id: editor.workspace_id })}>{t(language, "Task CLAUDE.md", "CLAUDE.md de tareas")}</button>
        </nav>
      )}
      <header className="document-header"><File size={16} /><div><strong>{editor.file_name}</strong><span>{editor.relative_path}</span></div><span className={`save-state is-${editor.save_state}`}>{saveStateLabel(editor.save_state, language)}</span><button type="button" className="workspace-button" onClick={() => postNative({ type: "save_active_file" })}><Save size={13} />{t(language, "Save", "Guardar")}</button><button type="button" className="workspace-icon-button" onClick={() => postNative({ type: "close_file_editor" })}><X size={15} /></button></header>
      {editor.state === "loading" ? <div className="workspace-empty">{t(language, "Opening file…", "Abriendo archivo…")}</div> : editor.state === "error" ? <div className="workspace-error">{editor.error}</div> : (
        <MonacoSurface file={`${root}/${editor.relative_path}`} content={editor.content} requestId={editor.request_id} theme={theme} language={language} />
      )}
    </section>
  );
}

function VirtualDiff({ diff }: { diff: DiffData }) {
  const viewport = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const rowHeight = 24;
  const height = viewport.current?.clientHeight || 600;
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - 16);
  const end = Math.min(diff.rows.length, Math.ceil((scrollTop + height) / rowHeight) + 16);
  return (
    <div className="diff-viewport" ref={viewport} onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}>
      <div className="diff-spacer" style={{ height: `${diff.rows.length * rowHeight}px` }}>
        <div style={{ transform: `translateY(${start * rowHeight}px)` }}>
          {diff.rows.slice(start, end).map((row, index) => row.row_type === "hunk" ? (
            <div className="diff-row is-hunk" key={`${start + index}-h`}><span>{row.old_start} {row.header}</span><span>{row.new_start} {row.header}</span></div>
          ) : (
            <div className={`diff-row is-${row.kind}`} key={`${start + index}-l`}><span><i>{row.old_number ?? ""}</i><code>{row.kind === "added" ? "" : row.old_text}</code></span><span><i>{row.new_number ?? ""}</i><code>{row.kind === "deleted" ? "" : row.new_text}</code></span></div>
          ))}
        </div>
      </div>
    </div>
  );
}

function DiffView({ diff, language, theme }: { diff: DiffData; language: "en" | "es"; theme: AppTheme }) {
  const full = diff.original != null && diff.modified != null;
  return (
    <section className="file-editor-shell">
      <header className="document-header"><Code2 size={16} /><div><strong>{diff.file_name}</strong><span>{diff.relative_path}</span></div><b className={`diff-status is-${diff.change_kind}`}>{changeMarker[diff.change_kind]}</b><button type="button" className="workspace-icon-button" onClick={() => postNative({ type: "close_repository_diff" })}><X size={15} /></button></header>
      {!full && <div className="diff-head"><span>HEAD</span><span>{t(language, "WORKING TREE", "CAMBIOS LOCALES")}</span></div>}
      {diff.state === "loading" ? <div className="workspace-empty">{t(language, "Loading comparison…", "Cargando comparación…")}</div> : diff.state === "error" ? <div className="workspace-error">{diff.error}</div> : diff.state === "binary" ? <div className="workspace-empty">{t(language, "Binary files cannot be compared here", "Los archivos binarios no se pueden comparar aquí")}</div> : full ? <MonacoSurface file={diff.relative_path} content={diff.modified!} original={diff.original!} requestId={diff.request_id} theme={theme} language={language} /> : diff.state === "empty" ? <div className="workspace-empty">{t(language, "No textual changes to display", "No hay cambios de texto para mostrar")}</div> : <VirtualDiff diff={diff} />}
      {!full && diff.truncated && <footer className="diff-truncated">{t(language, "Large diff truncated at 20,000 rows", "Diff grande truncado a 20 000 filas")}</footer>}
    </section>
  );
}

function WorkbenchView({ data }: { data: WorkbenchData }) {
  const [explorerWidth, setExplorerWidth] = useState(() => {
    const saved = readStoredNumber("blackholes-workbench-explorer-width", 300);
    return Number.isFinite(saved) && saved >= 220 && saved <= 520 ? saved : 300;
  });
  return (
    <main className="workbench-page">
      <RepositoryExplorer explorer={data.explorer} language={data.language} width={explorerWidth} onResize={setExplorerWidth} />
      <div className="workbench-content">
        {data.diff ? <DiffView diff={data.diff} language={data.language} theme={data.theme} /> : data.editor ? <FileEditor editor={data.editor} language={data.language} theme={data.theme} root={data.explorer.root_path} /> : (
          <div className="workspace-empty workbench-welcome">
            <span><FileCode2 size={22} /></span>
            <strong>{data.explorer.root_label}</strong>
            <p>{t(data.language, "Select a file in the explorer to open it", "Selecciona un archivo del explorador para abrirlo")}</p>
          </div>
        )}
      </div>
    </main>
  );
}

export function WorkspaceSurface({ event }: { event: WorkspaceSurfaceEvent }) {
  if (event.surface === "unassigned-agent") {
    const data = event.data as UnassignedAgentData;
    return <main className="workbench-page"><div className="workbench-content"><div className="workspace-empty workbench-welcome">
      <span><FolderOpen size={22} /></span>
      <strong>{data.title}</strong>
      <p>{data.description}</p>
    </div></div></main>;
  }
  if (event.surface === "settings") return <SettingsView data={event.data as SettingsData} />;
  if (event.surface === "project-settings") return <ProjectSettingsView data={event.data as ProjectSettingsData} />;
  if (event.surface === "note") return <NoteView data={event.data as NoteData} />;
  return <WorkbenchView data={event.data as WorkbenchData} />;
}

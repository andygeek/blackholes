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
  RefreshCw,
  Rocket,
  Save,
  Search,
  Settings,
  SquareTerminal,
  X,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { postNative } from "../shared/native";
import { SidebarResizeHandle } from "../shared/SidebarResizeHandle";
import { MonacoSurface } from "./MonacoSurface";
import { RepositoryExplorer } from "./RepositoryExplorer";
import { applyAppTheme, type AppTheme } from "../shared/theme";
import { TaskDetails, ProjectOverview, type TaskDetailsData, type ProjectOverviewData } from "./TaskDetails";

export type SurfaceKind = "settings" | "project-settings" | "task-details" | "project-overview" | "workbench" | "home";

interface HomeData {
  description: string;
}

export interface WorkspaceSurfaceEvent {
  type: "workspace_surface";
  surface: SurfaceKind;
  theme: AppTheme;
  data: SettingsData | ProjectSettingsData | TaskDetailsData | ProjectOverviewData | WorkbenchData | HomeData;
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
  external_integrations?: {
    running: boolean;
    install_required: boolean;
    error?: string | null;
    profiles: Array<{
      client: string;
      config_path: string;
      configured: boolean;
      changed: boolean;
      error?: string | null;
      skill_warning?: string | null;
    }>;
  };
  usage_cards: UsageCard[];
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
  terminal_skip_permissions: boolean;
  project_instructions: string;
  project_revision: number;
  task_instructions: string;
  task_revision: number;
  error?: string | null;
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

const saveStateLabel = (state: "saved" | "saving" | "error", language: "en" | "es") => ({
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

type PreferencePage = "general" | "accounts" | "usage" | "mcps";

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
  }, [activePage, data.provider, data.auth_mode]);
  const providers: Choice[] = [
    { value: "claude", label: "Claude" }, { value: "codex", label: "Codex" },
    { value: "gemini", label: "Gemini" }, { value: "opencode", label: "OpenCode · Generic" },
  ];
  const providerControl = <PreferenceSelect label={t(language, "Agent provider", "Proveedor del agente")}
    choices={providers} value={data.provider} onChange={(provider) => postNative({ type: "set_agent_provider", provider })} />;
  const providerRow = <PreferenceRow title={t(language, "Agent provider", "Proveedor del agente")}
    description={t(language, "Uses the CLI installed on your computer. Install and update your chosen agent with its own tools.", "Usa el CLI instalado en tu computadora. Instala y actualiza el agente elegido con sus propias herramientas.")}>{providerControl}</PreferenceRow>;

  const pages: Array<{ id: PreferencePage; label: string; description: string; icon: LucideIcon; keywords: string; badge?: number; content: React.ReactNode }> = [
    {
      id: "general", label: t(language, "General", "General"), icon: Settings,
      description: t(language, "Make Blackholes feel at home.", "Personaliza la apariencia y el espacio de trabajo."),
      keywords: "appearance apariencia theme tema claro oscuro light dark language idioma English Español projects proyectos carpeta folder " + data.projects_root,
      content: <>
        {!data.git_available && <PreferenceGroup title={t(language, "Finish setup", "Completar instalación")}>
          <PreferenceRow title={t(language, "Git tools", "Herramientas de Git")} description={t(language,
            "Cloning repositories requires Apple's Command Line Tools. Install them, then check again.",
            "Para clonar repositorios necesitas las herramientas de Apple. Instálalas y vuelve a comprobar.")}>
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
          <PreferenceRow title={t(language, "Account source", "Origen de la cuenta")} description={t(language, "Choose the account profile for new terminal sessions. Existing sessions keep their profile.", "Elige el perfil de cuenta para las nuevas sesiones de terminal. Las sesiones existentes conservan su perfil.")}>
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
      id: "usage", label: t(language, "Usage", "Consumo"), icon: Database,
      description: t(language, "Plan limits reported by your provider.", "Límites del plan reportados por tu proveedor."),
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
          : t(language, "Limits belong to the selected provider account.", "Los límites corresponden a la cuenta seleccionada del proveedor.")}</p>
        <PreferenceGroup title={t(language, "Plan and limits", "Plan y límites")}>
          {data.usage_cards.length === 0 && <p className="preference-empty">{t(language, "No usage reported yet.", "Todavía no hay consumo reportado.")}</p>}
          {data.usage_cards.map((card, index) => <PreferenceRow key={`${index}-${card.label}`} title={card.label} description={card.detail}>
            <div className="preference-usage">
              <strong>{card.value}</strong>
              {typeof card.utilization === "number" && Number.isFinite(card.utilization) && <progress max={100} value={Math.min(100, Math.max(0, card.utilization))} aria-label={card.label} aria-valuetext={card.value + ". " + card.detail} />}
            </div>
          </PreferenceRow>)}
        </PreferenceGroup>
        <div className="preference-footnote">{data.usage_updated && <p>{data.usage_updated}</p>}</div>
      </>,
    },
    {
      id: "mcps", label: t(language, "MCP servers", "Servidores MCP"), icon: Cable,
      description: t(language, "Connect Blackholes to your terminal agents.", "Conecta Blackholes a tus agentes de terminal."),
      keywords: "mcp servers servidores conexiones integrations integraciones",
      content: <>
        <div className="preference-toolbar"><span>{data.provider_label}</span><button className="workspace-button" type="button" onClick={() => setActivePage("accounts")}>{t(language, "Manage account", "Administrar cuenta")}</button></div>
        <PreferenceGroup title={t(language, "Blackholes in your terminal agents", "Blackholes en tus agentes de terminal")}>
          <PreferenceRow title={t(language, "Automatic connection", "Conexión automática")} description={t(language,
            "Blackholes prepares its MCP connection for Codex and Claude Code when the app opens, including after updates. It can prepare the connection before you install either CLI. Open a new agent session to use it.",
            "Blackholes prepara su conexión MCP para Codex y Claude Code al abrir la app, también después de actualizar. Puede preparar la conexión antes de que instales los CLI. Abre una nueva sesión del agente para usarla.")}>
            <button type="button" className="workspace-button" disabled={data.external_integrations?.running}
              onClick={() => postNative({ type: "refresh_external_integrations" })}>
              <RefreshCw size={14} />{data.external_integrations?.running ? t(language, "Preparing…", "Preparando…") : t(language, "Refresh connection", "Actualizar conexión")}
            </button>
          </PreferenceRow>
          {data.external_integrations?.install_required && <p className="preference-footnote" role="status">{t(language,
            "Move Blackholes to Applications and open it there to finish connecting your terminal agents.",
            "Mueve Blackholes a Aplicaciones y ábrelo desde allí para terminar de conectar tus agentes de terminal.")}</p>}
          {data.external_integrations?.error && <p className="preference-footnote" role="status">{data.external_integrations.error}</p>}
          {data.external_integrations?.profiles.map((profile) => <PreferenceRow key={profile.config_path} title={profile.client}
            description={<>
              {(profile.error || profile.skill_warning) && <span>{profile.error || profile.skill_warning}</span>}
              <details className="preference-item-details"><summary>{t(language, "Configuration location", "Ubicación de la configuración")}</summary><code>{profile.config_path}</code></details>
            </>}>
            <span className="preference-value">{profile.configured ? t(language, "Configured", "Configurado") : t(language, "Needs attention", "Requiere atención")}</span>
          </PreferenceRow>)}
        </PreferenceGroup>
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
          {(index === 0 || index === 1 || page.id === "mcps") && <span className="preferences-nav-label">{index === 0 ? t(language, "Application", "Aplicación") : index === 1 ? t(language, "Agents", "Agentes") : t(language, "Integrations", "Integraciones")}</span>}
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
        {visiblePages.length === 0 && <div className="preference-empty"><Search size={24} /><p>{t(language, "No matching settings. Try “account”, “language”, or “MCP”.", "No hay coincidencias. Prueba con «cuenta», «idioma» o «MCP».")}</p></div>}
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
  const [activeTab, setActiveTab] = useState<"instructions" | "terminals">("terminals");
  const tabs: SettingsTab<"instructions" | "terminals">[] = [
    { value: "instructions", label: t(language, "Agent instructions", "Instrucciones de agentes"), icon: FileCode2 },
    { value: "terminals", label: t(language, "Terminals", "Terminales"), icon: SquareTerminal },
  ];
  return (
    <main className="workspace-page settings-page project-settings-page">
      <div className="workspace-page__content">
        <header className="workspace-title">
          <span><Settings size={20} /></span>
          <div>
            <h1>{t(language, `Settings for ${data.title}`, `Configuración de ${data.title}`)}</h1>
            <p>{t(language, "Manage terminal permissions and instructions for this project.", "Administra los permisos de terminales y las instrucciones de este proyecto.")}</p>
          </div>
        </header>

        {data.error && <div className="workspace-inline-error">{data.error}</div>}

        <SettingsTabs
          tabs={tabs}
          value={activeTab}
          onChange={(value) => setActiveTab(value as typeof activeTab)}
          label={t(language, "Project settings sections", "Secciones de configuración del proyecto")}
        />

        {activeTab === "terminals" && (
          <div className="settings-tab-panel">
            <SettingsSection wide title={t(language, "Agent permissions", "Permisos de agentes")}
              description={t(language, "For terminal agents launched or restored in this project and its tasks, including parallel launches through MCP.", "Para los agentes de terminal que se inician o restauran en este proyecto y sus tareas, incluidos los inicios en paralelo por MCP.")}>
              <label className="project-terminal-permissions">
                <input type="checkbox" checked={Boolean(data.terminal_skip_permissions)}
                  aria-describedby="terminal-permissions-description"
                  onChange={event => postNative({ type: "set_project_terminal_skip_permissions", workspace_id: data.workspace_id, enabled: event.target.checked })} />
                <span><strong>{t(language, "Start agents without permission prompts", "Iniciar agentes sin pedir permisos")}</strong>
                  <p>{t(language, "Off by default. Changes apply the next time a terminal agent starts, including after reopening Blackholes.", "Desactivado por defecto. Se aplica la próxima vez que se inicie un agente de terminal, incluso al volver a abrir Blackholes.")}</p>
                </span>
              </label>
              <p id="terminal-permissions-description">{t(language,
                "Use only with trusted projects: agents can edit files and run commands without confirmation. Codex also disables its sandbox. Running agents and manually typed commands are unchanged. When off, Blackholes adds no bypass flags; your provider settings still apply.",
                "Úsalo solo en proyectos de confianza: los agentes podrán modificar archivos y ejecutar comandos sin confirmación. Codex también desactiva su sandbox. No cambia agentes activos ni comandos escritos manualmente. Al desactivarlo, Blackholes no añade flags de omisión; sigue aplicándose la configuración del proveedor.")}</p>
              <details className="project-terminal-permission-flags">
                <summary>{t(language, "Flags by agent", "Flags por agente")}</summary>
                <ul>
                  <li>Claude Code / Antigravity (agy): <code>--dangerously-skip-permissions</code></li>
                  <li>Codex: <code>--dangerously-bypass-approvals-and-sandbox</code></li>
                  <li>OpenCode: <code>--auto</code> · {t(language, "Explicit deny rules remain enforced.", "Conserva las reglas explícitas de denegación.")}</li>
                  <li>Gemini: <code>--approval-mode=yolo</code></li>
                </ul>
              </details>
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
  if (event.surface === "home") {
    const data = event.data as HomeData;
    return <main className="workbench-page"><div className="workbench-content"><div className="workspace-empty workbench-welcome home-welcome">
      <img className="home-welcome-logo" src={document.getElementById("root")?.dataset.appLogo} alt="Blackholes" width={112} height={112} draggable={false} />
      <p>{data.description}</p>
    </div></div></main>;
  }
  if (event.surface === "settings") return <SettingsView data={event.data as SettingsData} />;
  if (event.surface === "project-settings") return <ProjectSettingsView data={event.data as ProjectSettingsData} />;
  if (event.surface === "task-details") return <TaskDetails key={(event.data as TaskDetailsData).id} data={event.data as TaskDetailsData} />;
  if (event.surface === "project-overview") return <ProjectOverview data={event.data as ProjectOverviewData} />;
  return <WorkbenchView data={event.data as WorkbenchData} />;
}

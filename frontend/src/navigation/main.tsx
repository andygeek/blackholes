import { StrictMode, createContext, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { GroupIcon } from "../shared/GroupIcon";
import {
  ChevronRight,
  ChevronsDownUp,
  ChevronsUpDown,
  FilePenLine,
  FolderInput,
  FolderMinus,
  FolderPlus,
  GitBranch,
  ListTodo,
  MoreHorizontal,
  Pencil,
  Pin,
  PinOff,
  NotebookPen,
  ChevronsUp,
  Plus,
  RefreshCw,
  Settings,
  SquareTerminal,
  Trash2,
  X,
  type LucideIcon,
} from "lucide-react";
import { TerminalProviderIcon } from "../shared/TerminalProviderIcon";
import { SidebarResizeHandle } from "../shared/SidebarResizeHandle";
import { SidebarScrollArea } from "./SidebarScrollArea";
import { useAgentDrag, type DragItem, type DropTarget } from "./useAgentDrag";
import { ResizableSidebarSections } from "./ResizableSidebarSections";
import { NavigationStatusIcon, type NoticeKind } from "./NavigationStatusIcon";
import { postNative, type NativeCommand } from "../shared/native";
import { applyAppTheme, type AppTheme } from "../shared/theme";

interface Copy {
  projects: string;
  project: string;
  settings: string;
  working: string;
  backgroundTask: string;
  backgroundTasks: string;
  needsAttention: string;
  responseReady: string;
  terminal: string;
  blankTerminal: string;
  tasks: string;
  task: string;
  new: string;
  toggle: string;
  options: string;
  closeTerminal: string;
  rename: string;
  newSection: string;
  newSectionWithSession: string;
  moveToSection: string;
  removeFromSection: string;
  editSection: string;
  newSubsection: string;
  deleteSection: string;
  collapseSections: string;
  expandSections: string;
  emptySection: string;
  dropOutOfSection: string;
  unusedDays: string;
  pin: string;
  unpin: string;
  pinned: string;
  resumeSession: string;
  newTerminal: string;
  newTask: string;
  refreshProject: string;
  addToProject: string;
  addRepository: string;
  removeRepository: string;
  editProject: string;
  projectSettings: string;
  removeProject: string;
  editTask: string;
  removeTask: string;
}

interface TerminalItem {
  id: string;
  label: string;
  agent: string;
  selected: boolean;
  context: string;
  provider_label: string;
  project_label: string;
  group_title: string | null;
  group_icon: string | null;
  group_color: string | null;
  repository_label?: string | null;
  section_id?: string | null;
  note?: string | null;
  /** The user named the session; its label is shown exactly as typed. */
  custom_label?: boolean;
  /** Last time the user opened the session or its agent was active; null while unknown. */
  last_active_at?: string | null;
  needs_attention: boolean;
  attention_kind: NoticeKind | null;
  attention_rank: number | null;
  working: boolean;
  background_tasks?: number;
  pinned?: boolean;
  pin_rank?: number | null;
  live?: boolean;
}

interface RepositoryItem {
  id: string;
  name: string;
  branch?: string | null;
  additions?: number;
  deletions?: number;
  loading?: boolean;
  selected: boolean;
  terminals: TerminalItem[];
}

interface TaskItem {
  id: string;
  title: string;
  icon: string;
  color: string;
  expanded: boolean;
  selected: boolean;
  unseen: boolean;
  terminals: TerminalItem[];
  repositories: RepositoryItem[];
}

interface ProjectItem {
  id: string;
  label: string;
  icon: string;
  color: string;
  expanded: boolean;
  selected: boolean;
  terminals: TerminalItem[];
  repositories: RepositoryItem[];
  tasks: TaskItem[];
}

interface NavigationState {
  type: "hydrate";
  language: "en" | "es";
  theme: AppTheme;
  copy: Copy;
  settings_selected: boolean;
  sidebar_width: number;
  sidebar_agents_fraction: number;
  agents_section_collapsed: boolean;
  projects_section_collapsed: boolean;
  terminal_agents: TerminalItem[];
  agent_sections?: AgentSectionItem[];
  agent_order?: string[];
  agents?: LaunchAgent[];
  projects: ProjectItem[];
}

interface LaunchAgent {
  id: string;
  label: string;
}

/** A named section of the Agents list; it never changes the project tree. */
interface AgentSectionItem {
  id: string;
  name: string;
  collapsed: boolean;
  /** The section that contains this one; missing at the top of the list. */
  parent_id?: string | null;
}

/** A section, its sub-sections and theirs, as the app allows. */
const MAX_SECTION_DEPTH = 3;
const DAY_MS = 86_400_000;

/** The time used to color sessions nobody opened in a while; it advances on its own. */
const Now = createContext(Date.now());

/** Days since the user last opened the session or its agent was active; null while unknown. */
function unusedDays(terminal: TerminalItem, now: number) {
  const time = terminal.last_active_at ? Date.parse(terminal.last_active_at) : NaN;
  return Number.isNaN(time) ? null : Math.max(0, Math.floor((now - time) / DAY_MS));
}

/** Red after a week without use, black after a month. */
const staleClass = (days: number | null) => days === null ? "" : days >= 30 ? " is-stale-month" : days >= 7 ? " is-stale-week" : "";
const staleLabel = (days: number | null, copy: Copy) => days !== null && days >= 7 ? copy.unusedDays.replace("{count}", String(days)) : "";

/** The agents the user works with, chosen in Settings: each has an entry in the launch menus. */
const defaultLaunchAgents: LaunchAgent[] = [{ id: "claude", label: "Claude" }, { id: "codex", label: "Codex" }];
const LaunchAgents = createContext(defaultLaunchAgents);

interface MenuItem {
  label: string;
  icon: LucideIcon;
  provider?: string;
  danger?: boolean;
  command: NativeCommand;
}

interface MenuState {
  anchor: DOMRect;
  items: MenuItem[];
}

const fallbackCopy: Copy = {
  projects: "Proyectos",
  project: "Proyecto",
  settings: "Configuración",
  working: "Trabajando",
  backgroundTask: "1 tarea en segundo plano",
  backgroundTasks: "{count} tareas en segundo plano",
  needsAttention: "Necesita tu atención",
  responseReady: "Respuesta lista",
  terminal: "Terminal",
  blankTerminal: "Terminal vacía",
  tasks: "Tareas",
  task: "Tarea",
  new: "Nuevo",
  toggle: "Expandir o contraer",
  options: "Opciones",
  closeTerminal: "Cerrar terminal",
  rename: "Renombrar",
  newSection: "Nueva sección",
  newSectionWithSession: "Nueva sección…",
  moveToSection: "Mover a {name}",
  removeFromSection: "Quitar de {name}",
  editSection: "Editar sección",
  newSubsection: "Nueva subsección…",
  deleteSection: "Eliminar sección",
  collapseSections: "Contraer todas las secciones",
  expandSections: "Expandir todas las secciones",
  emptySection: "Sin sesiones",
  dropOutOfSection: "Suelta aquí para sacarla de su sección",
  unusedDays: "Sin abrir hace {count} días",
  pin: "Fijar arriba",
  unpin: "Desfijar",
  pinned: "Fijada",
  resumeSession: "Detenida · clic para reanudar",
  newTerminal: "Nueva terminal",
  newTask: "Agregar tarea",
  refreshProject: "Buscar repositorios nuevos",
  addToProject: "Agregar al proyecto",
  addRepository: "Agregar repositorio…",
  removeRepository: "Eliminar repositorio",
  editProject: "Editar proyecto",
  projectSettings: "Configuración del proyecto",
  removeProject: "Eliminar proyecto",
  editTask: "Editar tarea",
  removeTask: "Cerrar tarea",
};

const terminalNotice = (terminal: TerminalItem): NoticeKind | null => terminal.needs_attention
  ? terminal.attention_kind ?? "input-required" : null;

const noticeLabel = (notice: NoticeKind | null, copy: Copy) => notice === "completed"
  ? copy.responseReady : notice === "input-required" ? copy.needsAttention : "";

// Tasks the agent left running behind its last turn: marked in blue, never announced.
const runsInBackground = (terminal: TerminalItem) => terminal.agent !== "shell" && (terminal.background_tasks ?? 0) > 0;
const anyInBackground = (terminals: TerminalItem[]) => terminals.some(runsInBackground);

const terminalStatus = (terminal: TerminalItem, copy: Copy) => [
  noticeLabel(terminalNotice(terminal), copy) || (terminal.working ? copy.working : ""),
  !runsInBackground(terminal) ? "" : terminal.background_tasks === 1
    ? copy.backgroundTask : copy.backgroundTasks.replace("{count}", String(terminal.background_tasks)),
].filter(Boolean).join(" · ");

function pendingNotice(terminals: TerminalItem[]): NoticeKind | null {
  let result: NoticeKind | null = null;
  for (const terminal of terminals) {
    if (terminal.agent === "shell") continue;
    const notice = terminalNotice(terminal);
    if (notice === "input-required") return notice;
    if (notice) result = notice;
  }
  return result;
}

const taskTerminals = (task: TaskItem) => [...task.terminals, ...task.repositories.flatMap(repository => repository.terminals)];

/** The session's own name: the user's name or the agent's title. The provider is left out because
 * the icon already shows it, and so are the status glyphs agents put first (✳, a spinner). A title
 * that only names the provider, the task or the project is no name. */
function sessionName(terminal: TerminalItem) {
  if (terminal.custom_label) return terminal.label;
  const prefix = `${terminal.provider_label} · `;
  const name = (terminal.label.startsWith(prefix) ? terminal.label.slice(prefix.length) : terminal.label)
    .replace(/^[^\p{L}\p{N}]+/u, "").trim();
  return [terminal.provider_label, terminal.group_title, terminal.project_label].includes(name) ? "" : name;
}

/** Where the session runs: project or project/task, then its repository. */
const sessionPlace = (terminal: TerminalItem) =>
  `${terminal.context}${terminal.repository_label ? ` · ${terminal.repository_label}` : ""}`;

function TerminalAgentRow({ terminal, copy, onOpen, onMenu }: {
  terminal: TerminalItem; copy: Copy; onOpen(terminal: TerminalItem): void; onMenu(anchor: DOMRect, terminal: TerminalItem): void;
}) {
  // Listed sessions stay while stopped, also after a restart; a click resumes them.
  const dormant = terminal.live === false && !terminal.needs_attention;
  const status = dormant ? copy.resumeSession : terminalStatus(terminal, copy);
  const now = useContext(Now);
  const days = unusedDays(terminal, now);
  const unused = staleLabel(days, copy);
  const name = sessionName(terminal);
  const place = sessionPlace(terminal);
  const description = [name, place, terminal.provider_label, terminal.note].filter(Boolean).join(" · ");
  const placeLabel = <>
    {terminal.group_title ? <>
      {terminal.project_label}/<span className="agent-group-icon" style={{ color: terminal.group_color || undefined }}>
        <GroupIcon value={terminal.group_icon || "layers"} size={name ? 11 : 12} />
      </span>{terminal.group_title}
    </> : terminal.context}
    {terminal.repository_label && ` · ${terminal.repository_label}`}
  </>;
  // Without a name, the place moves up so the card never shows the provider in words.
  const subtitle = name ? <>{placeLabel}{dormant && ` · ${copy.resumeSession}`}</> : dormant ? copy.resumeSession : null;
  return (
    <div className={`agent-card terminal-agent-card${terminal.selected ? " is-selected" : ""}${terminal.needs_attention ? " needs-attention" : runsInBackground(terminal) ? " has-background" : ""}${terminal.attention_kind === "input-required" ? " is-awaiting-input" : ""}${terminal.pinned ? " is-pinned" : ""}${dormant ? " is-dormant" : ""}${staleClass(days)}`}
      onContextMenu={event => {
        event.preventDefault();
        onMenu(new DOMRect(event.clientX, event.clientY, 0, 0), terminal);
      }}>
      <button type="button" className="agent-open"
        aria-current={terminal.selected ? "true" : undefined}
        aria-label={`${description}${status ? ` · ${status}` : ""}${unused ? ` · ${unused}` : ""}`}
        title={`${name ? `${name}\n` : ""}${place}\n${terminal.provider_label}${terminal.note ? `\n${terminal.note}` : ""}${status ? `\n${status}` : ""}${unused ? `\n${unused}` : ""}`}
        onClick={() => onOpen(terminal)}>
        <NavigationStatusIcon notice={terminalNotice(terminal)} working={terminal.working} background={runsInBackground(terminal)} large>
          <TerminalProviderIcon provider={terminal.agent} />
        </NavigationStatusIcon>
        <span className="agent-copy">
          <span className="agent-name-line"><span className="agent-name">{name || placeLabel}</span></span>
          {subtitle && <span className="agent-preview">{subtitle}</span>}
        </span>
      </button>
      {terminal.pinned && <button type="button" className="pin-button"
        aria-label={`${copy.unpin}: ${description}`} title={`${copy.pinned} · ${copy.unpin}`}
        onClick={() => postNative({ type: "set_terminal_pinned", terminal_id: terminal.id, pinned: false })}>
        <Pin size={13} />
      </button>}
      <button type="button" className="remove-button"
        aria-label={`${copy.closeTerminal}: ${description}`}
        title={copy.closeTerminal}
        onClick={() => postNative({ type: "close_terminal", terminal_id: terminal.id })}>
        <X size={14} />
      </button>
    </div>
  );
}

function Chevron({ expanded, copy, command }: { expanded: boolean; copy: Copy; command: NativeCommand }) {
  return (
    <button
      type="button"
      className={`chevron${expanded ? " is-expanded" : ""}`}
      aria-label={copy.toggle}
      onClick={(event) => {
        event.stopPropagation();
        postNative(command);
      }}
    >
      <ChevronRight size={14} />
    </button>
  );
}

function RowAction({ label, icon: Icon, onClick }: { label: string; icon: LucideIcon; onClick(event: React.MouseEvent<HTMLButtonElement>): void }) {
  return (
    <button
      type="button"
      className="row-action"
      aria-label={label}
      title={label}
      onClick={(event) => {
        event.stopPropagation();
        onClick(event);
      }}
    >
      <Icon size={14} />
    </button>
  );
}

function Changes({ item }: { item: RepositoryItem }) {
  if (!item.additions && !item.deletions) return null;
  return (
    <span className="changes">
      {!!item.additions && <span className="changes__add">+{item.additions}</span>}
      {!!item.deletions && <span className="changes__delete">−{item.deletions}</span>}
    </span>
  );
}

function TerminalRow({ terminal, copy, indentation }: { terminal: TerminalItem; copy: Copy; indentation: number }) {
  const status = terminalStatus(terminal, copy);
  // Plain shells are not agents; only agent sessions turn red or black with time.
  const now = useContext(Now);
  const days = terminal.agent === "shell" ? null : unusedDays(terminal, now);
  const unused = staleLabel(days, copy);
  return (
    <button
      type="button"
      className={`tree-leaf terminal-tree-leaf tree-indent-${indentation}${terminal.selected ? " is-selected" : ""}${terminal.attention_kind === "input-required" ? " is-awaiting-input" : ""}${staleClass(days)}`}
      id={`nav-terminal-${terminal.id}`}
      aria-label={`${terminal.label}${status ? ` · ${status}` : ""}${unused ? ` · ${unused}` : ""}`}
      title={`${terminal.label}${status ? ` · ${status}` : ""}${unused ? `\n${unused}` : ""}`}
      onClick={() => postNative({ type: "focus_terminal", terminal_id: terminal.id })}
    >
      <NavigationStatusIcon notice={terminalNotice(terminal)} working={terminal.working} background={runsInBackground(terminal)}>
        <TerminalProviderIcon provider={terminal.agent} />
      </NavigationStatusIcon>
      <span className="tree-label">
        <span className="agent-name-line">
          <span className="tree-label">{terminal.label}</span>
        </span>
      </span>
      <button
        type="button"
        className="remove-button"
        aria-label={copy.closeTerminal}
        onClick={(event) => {
          event.stopPropagation();
          postNative({ type: "close_terminal", terminal_id: terminal.id });
        }}
      >
        <X size={13} />
      </button>
    </button>
  );
}

function terminalLaunchItems(workspaceId: string, taskId: string | null | undefined, repositoryId: string | null, copy: Copy, agents: LaunchAgent[]): MenuItem[] {
  return [
    ["shell", copy.blankTerminal], ...agents.map(agent => [agent.id, agent.label]),
  ].map(([agent, label]) => ({
    label, icon: SquareTerminal, provider: agent === "shell" ? undefined : agent,
    command: { type: "new_terminal", workspace_id: workspaceId, task_id: taskId || null, repository_id: repositoryId, agent },
  }));
}

function RepositoryRow({ repository, workspaceId, taskId, copy, indentation, openMenu }: {
  repository: RepositoryItem;
  workspaceId: string;
  taskId?: string;
  copy: Copy;
  indentation: number;
  openMenu(anchor: DOMRect, items: MenuItem[]): void;
}) {
  const agents = useContext(LaunchAgents);
  return (
    <>
      <button
        type="button"
        className={`tree-leaf tree-indent-${indentation}${repository.selected ? " is-selected" : ""}`}
        aria-label={repository.name}
        onClick={() => postNative({
          type: "select_repository",
          workspace_id: workspaceId,
          task_id: taskId || null,
          repository_id: repository.id,
        })}
      >
        <span className="row-icon"><GitBranch size={13} /></span>
        <span className="tree-label">{repository.name}</span>
        {repository.branch && <span className="tree-meta">{repository.branch}</span>}
        <Changes item={repository} />
        <span className="row-actions">
          <RowAction
            label={copy.newTerminal}
            icon={Plus}
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), terminalLaunchItems(workspaceId, taskId, repository.id, copy, agents))}
          />
          {!taskId && <RowAction label={copy.options} icon={MoreHorizontal}
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), [
              { label: copy.removeRepository, icon: Trash2, danger: true, command: { type: "remove_repository", workspace_id: workspaceId, repository_id: repository.id } },
            ])} />}
        </span>
      </button>
      {repository.terminals.map((terminal) => (
        <TerminalRow key={terminal.id} terminal={terminal} copy={copy} indentation={indentation + 1} />
      ))}
    </>
  );
}

function TaskRows({ task, workspaceId, copy, openMenu }: {
  task: TaskItem;
  workspaceId: string;
  copy: Copy;
  openMenu(anchor: DOMRect, items: MenuItem[]): void;
}) {
  const agents = useContext(LaunchAgents);
  const terminals = taskTerminals(task);
  const notice = pendingNotice(terminals);
  const status = noticeLabel(notice, copy);
  return (
    <>
      <button
        type="button"
        className={`tree-row tree-indent-1${task.selected ? " is-selected" : ""}`}
        id={`nav-task-${task.id}`}
        style={{ "--row-color": task.color } as React.CSSProperties}
        aria-label={`${task.title}${status ? ` · ${status}` : ""}`}
        title={`${task.title}${status ? ` · ${status}` : ""}`}
        onClick={() => postNative({ type: "select_task", workspace_id: workspaceId, task_id: task.id })}
      >
        <Chevron expanded={task.expanded} copy={copy} command={{ type: "toggle_task", task_id: task.id }} />
        <NavigationStatusIcon notice={notice} backgroundInside={anyInBackground(terminals)}><GroupIcon value={task.icon} size={13} /></NavigationStatusIcon>
        <span className="tree-label">{task.title}</span>
        {task.unseen && <span className="new-badge">{copy.new}</span>}
        <span className="row-actions">
          <RowAction
            label={copy.newTerminal}
            icon={Plus}
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), terminalLaunchItems(workspaceId, task.id, null, copy, agents))}
          />
          <RowAction
            label={copy.options}
            icon={MoreHorizontal}
            onClick={(event) => {
              const items: MenuItem[] = [
                { label: copy.editTask, icon: FilePenLine, command: { type: "edit_task", task_id: task.id } },
                { label: copy.removeTask, icon: Trash2, danger: true, command: { type: "remove_task", task_id: task.id } },
              ];
              openMenu(event.currentTarget.getBoundingClientRect(), items);
            }}
          />
        </span>
      </button>
      {task.expanded && (
        <>
          {task.terminals.map((terminal) => <TerminalRow key={terminal.id} terminal={terminal} copy={copy} indentation={2} />)}
          {task.repositories.map((repository) => (
            <RepositoryRow
              key={repository.id}
              repository={repository}
              workspaceId={workspaceId}
              taskId={task.id}
              copy={copy}
              indentation={2}
              openMenu={openMenu}
            />
          ))}
        </>
      )}
    </>
  );
}

function ProjectBlock({ project, copy, openMenu }: {
  project: ProjectItem;
  copy: Copy;
  openMenu(anchor: DOMRect, items: MenuItem[]): void;
}) {
  const agents = useContext(LaunchAgents);
  const terminals = [
    ...project.terminals,
    ...project.repositories.flatMap(repository => repository.terminals),
    ...project.tasks.flatMap(taskTerminals),
  ];
  const notice = pendingNotice(terminals);
  const status = noticeLabel(notice, copy);
  return (
    <section className="project-block">
      <button
        type="button"
        className={`tree-row${project.selected ? " is-selected" : ""}`}
        id={`nav-project-${project.id}`}
        style={{ "--row-color": project.color } as React.CSSProperties}
        aria-label={`${project.label}${status ? ` · ${status}` : ""}`}
        title={`${project.label}${status ? ` · ${status}` : ""}`}
        onClick={() => postNative({ type: "select_project", workspace_id: project.id })}
      >
        <Chevron expanded={project.expanded} copy={copy} command={{ type: "toggle_project", workspace_id: project.id }} />
        <NavigationStatusIcon notice={notice} backgroundInside={anyInBackground(terminals)}><GroupIcon value={project.icon || "layers"} size={13} /></NavigationStatusIcon>
        <span className="tree-label">{project.label}</span>
        <span className="row-actions">
          <RowAction
            label={copy.refreshProject}
            icon={RefreshCw}
            onClick={() => postNative({ type: "refresh_project", workspace_id: project.id })}
          />
          <RowAction
            label={copy.addToProject}
            icon={Plus}
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), [
              ...terminalLaunchItems(project.id, null, null, copy, agents),
              { label: copy.addRepository, icon: Plus, command: { type: "add_project_repository", workspace_id: project.id } },
              { label: copy.newTask, icon: ListTodo, command: { type: "new_task", workspace_id: project.id } },
            ])}
          />
          <RowAction
            label={copy.options}
            icon={MoreHorizontal}
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), [
              { label: copy.editProject, icon: FilePenLine, command: { type: "edit_project", workspace_id: project.id } },
              { label: copy.projectSettings, icon: Settings, command: { type: "project_settings", workspace_id: project.id } },
              { label: copy.removeProject, icon: Trash2, danger: true, command: { type: "remove_project", workspace_id: project.id } },
            ])}
          />
        </span>
      </button>
      {project.expanded && (
        <>
          {project.terminals.map((terminal) => <TerminalRow key={terminal.id} terminal={terminal} copy={copy} indentation={1} />)}
          {project.repositories.map((repository) => (
            <RepositoryRow
              key={repository.id}
              repository={repository}
              workspaceId={project.id}
              copy={copy}
              indentation={1}
              openMenu={openMenu}
            />
          ))}
          {project.tasks.length > 0 && <div className="tasks-label">{copy.tasks.toUpperCase()}</div>}
          {project.tasks.map((task) => (
            <TaskRows key={task.id} task={task} workspaceId={project.id} copy={copy} openMenu={openMenu} />
          ))}
        </>
      )}
    </section>
  );
}

function ContextMenu({ menu, close }: { menu: MenuState; close(): void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 8, top: 8 });

  useLayoutEffect(() => {
    const bounds = ref.current?.getBoundingClientRect();
    if (!bounds) return;
    setPosition({
      left: Math.max(8, Math.min(menu.anchor.right - bounds.width, innerWidth - bounds.width - 8)),
      top: Math.max(8, Math.min(menu.anchor.bottom + 3, innerHeight - bounds.height - 8)),
    });
  }, [menu]);

  return (
    <div ref={ref} className="context-menu" role="menu" style={position} onClick={(event) => event.stopPropagation()}>
      {menu.items.map((item, index) => {
        const Icon = item.icon;
        return (
          <button
            key={`${item.label}-${index}`}
            type="button"
            className={`menu-item${item.danger ? " is-danger" : ""}`}
            onClick={() => {
              close();
              postNative(item.command);
            }}
          >
            <span className="menu-item__icon">{item.provider ? <TerminalProviderIcon provider={item.provider} /> : <Icon size={14} />}</span>
            <span className="menu-item__label">{item.label}</span>
          </button>
        );
      })}
    </div>
  );
}

/** Centers a row in the Projects viewport; never moves the Agents list or the whole WebView. */
function centerInProjects(row: HTMLElement) {
  const viewport = row.closest<HTMLElement>(".sidebar-scroll");
  if (!viewport) return false;
  const bounds = row.getBoundingClientRect();
  const viewportBounds = viewport.getBoundingClientRect();
  viewport.scrollTo({
    top: viewport.scrollTop + bounds.top - viewportBounds.top - (viewport.clientHeight - bounds.height) / 2,
    behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth",
  });
  return true;
}

function NavigationApp() {
  const [state, setState] = useState<NavigationState | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [modalVisible, setModalVisible] = useState(false);
  const [revealTarget, setRevealTarget] = useState<{ id: string } | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const revealTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const revealedRow = useRef<HTMLElement | null>(null);
  const followedRow = useRef<string | null>(null);
  const copy = state?.copy || fallbackCopy;
  const agentsCollapsed = state?.agents_section_collapsed ?? false;
  const projectsCollapsed = state?.projects_section_collapsed ?? false;
  const agentsLabel = state?.language === "en" ? "Agents" : "Agentes";

  const setSectionCollapsed = (section: "agents" | "projects", collapsed: boolean) => {
    setMenu(null);
    setState(previous => previous ? { ...previous, [`${section}_section_collapsed`]: collapsed } : previous);
    postNative({ type: "set_sidebar_section_collapsed", section, collapsed });
  };

  useLayoutEffect(() => {
    if (!revealTarget || projectsCollapsed) return;
    const row = document.getElementById(revealTarget.id);
    if (!row) return; // A hydrate may still be expanding the parent sections.
    if (revealTimer.current) clearTimeout(revealTimer.current);
    revealedRow.current?.classList.remove("is-revealed");
    if (!centerInProjects(row)) return;
    row.classList.add("is-revealed");
    revealedRow.current = row;
    revealTimer.current = setTimeout(() => row.classList.remove("is-revealed"), 1800);
    setRevealTarget(null);
  }, [state, revealTarget, projectsCollapsed]);

  // The open session keeps its row in view in Projects, whatever opened it: a click in Agents,
  // a notification, a link, the keyboard or the MCP. Only a change of session scrolls, so
  // scrolling by hand is never undone.
  useLayoutEffect(() => {
    if (!state || projectsCollapsed) return;
    const row = document.querySelector<HTMLElement>(".terminal-tree-leaf.is-selected");
    const id = row?.id ?? null;
    if (id === followedRow.current) return;
    followedRow.current = id;
    // A reveal of the same row centers and highlights it already.
    if (!row || revealTarget?.id === id) return;
    const viewport = row.closest<HTMLElement>(".sidebar-scroll");
    const bounds = row.getBoundingClientRect();
    const viewportBounds = viewport?.getBoundingClientRect();
    if (viewportBounds && bounds.top >= viewportBounds.top && bounds.bottom <= viewportBounds.bottom) return;
    centerInProjects(row);
  }, [state, revealTarget, projectsCollapsed]);

  useEffect(() => () => {
    if (revealTimer.current) clearTimeout(revealTimer.current);
    revealedRow.current?.classList.remove("is-revealed");
  }, []);

  useEffect(() => {
    window.blackholesNavigation = {
      receive(event) {
        if (typeof event !== "object" || event === null) return;
        if ((event as { type?: string }).type === "modal_visibility") {
          setModalVisible(Boolean((event as { visible?: boolean }).visible));
          setMenu(null);
        } else if ((event as { type?: string }).type === "reveal_target") {
          const target = event as { workspace_id: string; task_id?: string | null };
          setMenu(null);
          setRevealTarget({ id: target.task_id ? `nav-task-${target.task_id}` : `nav-project-${target.workspace_id}` });
        } else if ((event as { type?: string }).type === "hydrate") {
          const next = event as NavigationState;
          document.documentElement.lang = next.language || "es";
          applyAppTheme(next.theme);
          setNow(Date.now());
          setState(next);
        }
      },
    };
    postNative({ type: "ready" });
    return () => { delete window.blackholesNavigation; };
  }, []);

  // Sessions turn red or black while the app stays open, without waiting for a change.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 10 * 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const close = () => setMenu(null);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        close();
      }
    };
    document.addEventListener("click", close);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("click", close);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  const openMenu = (anchor: DOMRect, items: MenuItem[]) => setMenu({ anchor, items });
  const openTerminalAgent = (terminal: TerminalItem) => {
    setRevealTarget({ id: `nav-terminal-${terminal.id}` });
    postNative({ type: "focus_terminal", terminal_id: terminal.id });
  };
  const agentRanks = new Map((state?.agent_order || []).map((id, index) => [id, index]));
  const manualAgentOrder = [...(state?.terminal_agents || [])].sort((a, b) =>
    (agentRanks.get(`terminal:${a.id}`) ?? Number.MAX_SAFE_INTEGER) - (agentRanks.get(`terminal:${b.id}`) ?? Number.MAX_SAFE_INTEGER));
  const agentsNotice = agentsCollapsed ? pendingNotice(state?.terminal_agents || []) : null;
  const agentsInBackground = agentsCollapsed && !agentsNotice && anyInBackground(state?.terminal_agents || []);
  const sections = state?.agent_sections || [];
  const sectionIds = new Set(sections.map(section => section.id));
  // A session whose section was just removed shows without one until the next hydrate.
  const sectionOf = (terminal: TerminalItem) => terminal.section_id && sectionIds.has(terminal.section_id) ? terminal.section_id : null;
  const findSection = (id: string | null | undefined) => sections.find(section => section.id === id);
  const parentOf = (section: AgentSectionItem | undefined) => section?.parent_id && section.parent_id !== section.id
    && sectionIds.has(section.parent_id) ? section.parent_id : null;
  const childrenOf = (parent: string | null) => sections.filter(section => parentOf(section) === parent);
  /** The section and every section inside it; a cycle saved by another build cannot loop. */
  const descendantsOf = (id: string) => {
    const ids = [id];
    for (let index = 0; index < ids.length; index++) {
      for (const child of childrenOf(ids[index])) if (!ids.includes(child.id)) ids.push(child.id);
    }
    return ids;
  };
  const ancestorsOf = (id: string | null) => {
    const ids: string[] = [];
    for (let current = parentOf(findSection(id)); current && !ids.includes(current); current = parentOf(findSection(current))) ids.push(current);
    return ids;
  };
  const depthOf = (id: string) => ancestorsOf(id).length;
  const pathOf = (id: string) => [...ancestorsOf(id).reverse(), id].map(section => findSection(section)?.name).filter(Boolean).join(" / ");
  const agentTerminals = state?.terminal_agents || [];
  const membersOf = (section: string | null) => agentTerminals.filter(terminal => sectionOf(terminal) === section);
  const allMembersOf = (section: string) => {
    const ids = new Set(descendantsOf(section));
    return agentTerminals.filter(terminal => ids.has(sectionOf(terminal) || ""));
  };
  /** Display order of one block: pins in pin order, then notices, working agents and the manual order. */
  const blockOrder = (terminals: TerminalItem[]) => {
    const members = new Set(terminals.map(terminal => terminal.id));
    const pinned = terminals.filter(terminal => terminal.pinned)
      .sort((a, b) => (a.pin_rank ?? Number.MAX_SAFE_INTEGER) - (b.pin_rank ?? Number.MAX_SAFE_INTEGER));
    const unpinned = manualAgentOrder.filter(terminal => !terminal.pinned && members.has(terminal.id)).sort((a, b) =>
      Number(b.needs_attention) - Number(a.needs_attention)
        || (a.needs_attention ? (a.attention_rank ?? Number.MAX_SAFE_INTEGER) - (b.attention_rank ?? Number.MAX_SAFE_INTEGER)
          : Number(b.working) - Number(a.working)));
    return [...pinned, ...unpinned];
  };
  const openAgentMenu = (anchor: DOMRect, terminal: TerminalItem) => {
    const current = findSection(sectionOf(terminal));
    openMenu(anchor, [terminal.pinned
      ? { label: copy.unpin, icon: PinOff, command: { type: "set_terminal_pinned", terminal_id: terminal.id, pinned: false } }
      : { label: copy.pin, icon: Pin, command: { type: "set_terminal_pinned", terminal_id: terminal.id, pinned: true } },
      { label: copy.rename, icon: Pencil, command: { type: "rename_terminal", terminal_id: terminal.id } },
      ...sections.filter(section => section !== current).map(section => ({
        label: copy.moveToSection.replace("{name}", pathOf(section.id)), icon: FolderInput,
        command: { type: "set_terminal_section", terminal_id: terminal.id, section_id: section.id },
      })),
      ...(current ? [{ label: copy.removeFromSection.replace("{name}", current.name), icon: FolderMinus,
        command: { type: "set_terminal_section", terminal_id: terminal.id, section_id: null } }] : []),
      { label: copy.newSectionWithSession, icon: FolderPlus, command: { type: "new_agent_section", terminal_id: terminal.id } },
      { label: copy.closeTerminal, icon: X, danger: true, command: { type: "close_terminal", terminal_id: terminal.id } }]);
  };
  const openSectionMenu = (anchor: DOMRect, section: AgentSectionItem) => openMenu(anchor, [
    { label: copy.editSection, icon: Pencil, command: { type: "rename_agent_section", section_id: section.id } },
    ...(depthOf(section.id) + 1 < MAX_SECTION_DEPTH ? [{ label: copy.newSubsection, icon: FolderPlus,
      command: { type: "new_agent_section", terminal_id: null, parent_id: section.id } }] : []),
    { label: copy.deleteSection, icon: Trash2, danger: true, command: { type: "delete_agent_section", section_id: section.id } },
  ]);
  const setSectionFolded = (section: AgentSectionItem, collapsed: boolean) => {
    setMenu(null);
    setState(previous => previous ? { ...previous, agent_sections: (previous.agent_sections || []).map(current =>
      current.id === section.id ? { ...current, collapsed } : current) } : previous);
    postNative({ type: "set_agent_section_collapsed", section_id: section.id, collapsed });
  };
  const anySectionOpen = sections.some(section => !section.collapsed);
  const setAllSectionsFolded = (collapsed: boolean) => {
    setMenu(null);
    setState(previous => previous ? { ...previous, agent_sections: (previous.agent_sections || []).map(current => ({ ...current, collapsed })) } : previous);
    postNative({ type: "set_all_agent_sections_collapsed", collapsed });
  };
  const [announcement, setAnnouncement] = useState("");
  const moveSession = (terminalId: string, section: string | null, order: string[]) => {
    postNative({ type: "move_agent_session", terminal_id: terminalId, section_id: section, order });
    const name = agentTerminals.find(terminal => terminal.id === terminalId);
    const place = section ? pathOf(section) : agentsLabel;
    setAnnouncement(`${name ? sessionName(name) || name.context : ""}: ${place} · ${order.indexOf(terminalId) + 1}/${order.length}`);
  };
  const placeSection = (id: string, parent: string | null, before: string | null) => {
    postNative({ type: "place_agent_section", section_id: id, parent_id: parent, before_id: before });
    setAnnouncement(`${findSection(id)?.name || ""}: ${parent ? pathOf(parent) : agentsLabel}`);
  };
  /** The section a dropped section would end up inside; null at the top of the list. */
  const dropParent = (target: DropTarget) => target.kind === "unsectioned" ? null
    : target.kind === "session-row" ? target.sectionId
    : target.position === "inside" ? target.sectionId : parentOf(findSection(target.sectionId));
  const drag = useAgentDrag({
    disabled: modalVisible,
    allows: (item: DragItem, target: DropTarget) => {
      if (item.kind === "session") return true;
      const moved = descendantsOf(item.id);
      if (target.kind === "section" && moved.includes(target.sectionId)) return false;
      const parent = dropParent(target);
      const height = Math.max(0, ...moved.map(id => depthOf(id) - depthOf(item.id)));
      return !(parent && moved.includes(parent)) && (parent ? depthOf(parent) + 1 : 0) + height < MAX_SECTION_DEPTH;
    },
    onDrop: (item: DragItem, target: DropTarget) => {
      if (item.kind === "session") {
        const section = target.kind === "session-row" ? target.sectionId : target.kind === "section" ? target.sectionId : null;
        const ids = blockOrder(membersOf(section)).map(terminal => terminal.id).filter(id => id !== item.id);
        const index = target.kind === "session-row" ? ids.indexOf(target.terminalId) : -1;
        const at = index >= 0 ? index + Number(target.kind === "session-row" && target.after)
          : target.kind === "unsectioned" ? 0 : ids.length;
        ids.splice(at, 0, item.id);
        moveSession(item.id, section, ids);
        return;
      }
      const parent = dropParent(target);
      const siblings = childrenOf(parent).filter(section => section.id !== item.id);
      const before = target.kind === "unsectioned" ? siblings[0]?.id ?? null
        : target.kind === "section" && target.position === "before" ? target.sectionId
        : target.kind === "section" && target.position === "after"
          ? siblings[siblings.findIndex(section => section.id === target.sectionId) + 1]?.id ?? null
          : null;
      placeSection(item.id, parent, before);
    },
    onMoveByKey: (item: DragItem, direction: -1 | 1) => {
      if (item.kind === "session") {
        const terminal = agentTerminals.find(terminal => terminal.id === item.id);
        if (!terminal) return;
        const section = sectionOf(terminal);
        const ids = blockOrder(membersOf(section)).map(terminal => terminal.id);
        const index = ids.indexOf(item.id), next = index + direction;
        if (index < 0 || next < 0 || next >= ids.length) return;
        [ids[index], ids[next]] = [ids[next], ids[index]];
        moveSession(item.id, section, ids);
        return;
      }
      const parent = parentOf(findSection(item.id));
      const siblings = childrenOf(parent);
      const index = siblings.findIndex(section => section.id === item.id);
      if (direction < 0 && index > 0) placeSection(item.id, parent, siblings[index - 1].id);
      if (direction > 0 && index >= 0 && index < siblings.length - 1) placeSection(item.id, parent, siblings[index + 2]?.id ?? null);
    },
  });
  const dropClass = (target: DropTarget | null) => !target ? ""
    : target.kind === "section" ? ` drop-${target.position}`
    : target.kind === "session-row" ? (target.after ? " drop-after" : " drop-before") : " drop-inside";
  /** Session cards of one block: no section, or the sessions directly in one section. */
  const agentBlock = (section: string | null, terminals: TerminalItem[]) => <div className="global-agents">
    {blockOrder(terminals).map(terminal => {
      const target = drag.target?.kind === "session-row" && drag.target.terminalId === terminal.id ? drag.target : null;
      return <div key={terminal.id} data-drag-session={terminal.id} data-section-id={section ?? ""}
        className={`sortable-agent${drag.dragging?.kind === "session" && drag.dragging.id === terminal.id ? " is-dragging" : ""}${dropClass(target)}`}>
        <TerminalAgentRow terminal={terminal} copy={copy} onOpen={openTerminalAgent} onMenu={openAgentMenu} />
      </div>;
    })}
  </div>;
  const renderSection = (section: AgentSectionItem, depth: number, rendered: Set<string>): React.ReactNode => {
    if (rendered.has(section.id)) return null;
    rendered.add(section.id);
    const members = membersOf(section.id);
    const all = allMembersOf(section.id);
    const children = childrenOf(section.id);
    const notice = section.collapsed ? pendingNotice(all) : null;
    const background = section.collapsed && !notice && anyInBackground(all);
    const target = drag.target?.kind === "section" && drag.target.sectionId === section.id ? drag.target : null;
    return <div key={section.id} className={`agent-section${drag.dragging?.kind === "section" && drag.dragging.id === section.id ? " is-dragging" : ""}`}
      style={{ "--section-depth": depth } as React.CSSProperties}>
      <div className={`agent-section-header${dropClass(target)}`} data-drag-section={section.id} onContextMenu={event => {
        event.preventDefault();
        openSectionMenu(new DOMRect(event.clientX, event.clientY, 0, 0), section);
      }}>
        <button type="button" className="agent-section-toggle" aria-expanded={!section.collapsed}
          aria-label={`${section.name} · ${all.length}${notice ? ` · ${noticeLabel(notice, copy)}` : ""}`}
          disabled={modalVisible} onClick={() => setSectionFolded(section, !section.collapsed)}>
          <ChevronRight size={13} aria-hidden="true" />
          <span className="agent-section-name">{section.name}</span>
          <span className="agent-section-count" aria-hidden="true">{all.length}</span>
          {(notice || background) && <span aria-hidden="true" className={`navigation-status-dot agents-summary-dot${notice === "input-required" ? " is-awaiting-input" : background ? " has-background" : ""}`} />}
        </button>
        <button type="button" className="row-action agent-section-options" aria-label={`${copy.options}: ${section.name}`} title={copy.options}
          disabled={modalVisible} onClick={event => {
            event.stopPropagation();
            openSectionMenu(event.currentTarget.getBoundingClientRect(), section);
          }}>
          <MoreHorizontal size={14} />
        </button>
      </div>
      {!section.collapsed && <>
        {members.length > 0 ? agentBlock(section.id, members)
          : children.length === 0 && <p className="agent-section-empty">{copy.emptySection}</p>}
        {children.map(child => renderSection(child, depth + 1, rendered))}
      </>}
    </div>;
  };
  const unsectioned = membersOf(null);

  return (
    <Now.Provider value={now}>
    <aside className="sidebar" aria-label="Blackholes">
      <SidebarResizeHandle width={state?.sidebar_width ?? 280} right={-1} hitWidth={4} label={state?.language === "en" ? "Resize sidebar" : "Cambiar ancho del menú lateral"} />
      <header className="brand" inert={modalVisible}>
        <button className="brand__name" type="button" onClick={() => postNative({ type: "show_home" })}>
          BLACKHOLES
        </button>
      </header>
      <ResizableSidebarSections fraction={state?.sidebar_agents_fraction ?? .4}
        agentsCollapsed={agentsCollapsed} projectsCollapsed={projectsCollapsed} disabled={modalVisible}
        label={state?.language === "en" ? "Drag up or down to resize Agents and Projects" : "Arrastra arriba o abajo para ajustar Agentes y Proyectos"}
        onResize={(fraction, commit) => postNative({ type: "set_sidebar_agents_fraction", fraction, commit })}>
        <section className="agents-shell" aria-label={agentsLabel}>
          <header className="section-header">
            <button className="section-toggle" type="button" aria-expanded={!agentsCollapsed}
              aria-label={`${agentsLabel}${agentsNotice ? ` · ${noticeLabel(agentsNotice, copy)}` : ""}`}
              aria-controls="sidebar-agents-content" onClick={() => setSectionCollapsed("agents", !agentsCollapsed)}>
              <ChevronRight size={14} aria-hidden="true" />
              <span>{agentsLabel}</span>
              {(agentsNotice || agentsInBackground) && <span aria-hidden="true" className={`navigation-status-dot agents-summary-dot${agentsNotice === "input-required" ? " is-awaiting-input" : agentsInBackground ? " has-background" : ""}`} />}
            </button>
            <span className="section-header__actions">
              {sections.length > 0 && <button className="icon-button" type="button"
                aria-label={anySectionOpen ? copy.collapseSections : copy.expandSections}
                title={anySectionOpen ? copy.collapseSections : copy.expandSections}
                onClick={() => {
                  if (agentsCollapsed) setSectionCollapsed("agents", false);
                  setAllSectionsFolded(anySectionOpen);
                }}>
                {anySectionOpen ? <ChevronsDownUp size={15} /> : <ChevronsUpDown size={15} />}
              </button>}
              <button className="icon-button" type="button" aria-label={copy.newSection} title={copy.newSection}
                onClick={() => {
                  if (agentsCollapsed) setSectionCollapsed("agents", false);
                  postNative({ type: "new_agent_section", terminal_id: null });
                }}>
                <FolderPlus size={15} />
              </button>
            </span>
          </header>
          <SidebarScrollArea id="sidebar-agents-content" hidden={agentsCollapsed}
            label={state?.language === "en" ? "Scroll agents" : "Desplazar agentes"}>
          <div ref={drag.container} className={`agents-tree${drag.dragging ? " is-reordering" : ""}`} {...drag.handlers}>
          {agentTerminals.length === 0 && sections.length === 0 && <p className="agents-empty">{state?.language === "en"
            ? "No agents yet. Open a saved session from Projects or start one with +. Sessions stay here until you close them."
            : "Todavía no hay agentes. Abre una sesión guardada desde Proyectos o inicia una con +. Las sesiones se quedan aquí hasta que las cierres."}</p>}
          {drag.dragging?.kind === "session" && unsectioned.length === 0 && <div data-drop-unsectioned=""
            className={`agents-drop-zone${drag.target?.kind === "unsectioned" ? " drop-inside" : ""}`}>{copy.dropOutOfSection}</div>}
          {unsectioned.length > 0 && agentBlock(null, unsectioned)}
          {(() => {
            const rendered = new Set<string>();
            return childrenOf(null).map(section => renderSection(section, 0, rendered));
          })()}
          <span className="agent-order-announcement" role="status" aria-live="polite">{announcement}</span>
          </div>
          </SidebarScrollArea>
        </section>
        <section className="projects-shell" aria-label={copy.projects}>
          <header className="section-header">
            <button className="section-toggle" type="button" aria-expanded={!projectsCollapsed}
              aria-controls="sidebar-projects-content" onClick={() => setSectionCollapsed("projects", !projectsCollapsed)}>
              <ChevronRight size={14} aria-hidden="true" />
              <span>{copy.projects}</span>
            </button>
            <span className="section-header__actions">
              <button className="icon-button" type="button" aria-label={state?.language === "en" ? "Collapse all projects and tasks" : "Contraer todos los proyectos y tareas"} title={state?.language === "en" ? "Collapse all projects and tasks" : "Contraer todos los proyectos y tareas"} onClick={() => postNative({ type: "collapse_all" })}>
                <ChevronsUp size={15} />
              </button>
              <button className="icon-button" type="button"
                aria-label={state?.language === "en" ? "New project" : "Nuevo proyecto"}
                title={state?.language === "en" ? "New project" : "Nuevo proyecto"}
                onClick={() => {
                  if (projectsCollapsed) setSectionCollapsed("projects", false);
                  postNative({ type: "new_project" });
                }}>
                <Plus size={17} />
              </button>
            </span>
          </header>
          <SidebarScrollArea id="sidebar-projects-content" hidden={projectsCollapsed}
            label={state?.language === "en" ? "Scroll projects" : "Desplazar proyectos"}>
          <nav className="projects" aria-label={copy.projects}>
            <LaunchAgents.Provider value={state?.agents ?? defaultLaunchAgents}>
            {(state?.projects || []).map((project) => (
              <ProjectBlock
                key={project.id}
                project={project}
                copy={copy}
                openMenu={openMenu}
              />
            ))}
            </LaunchAgents.Provider>
          </nav>
          </SidebarScrollArea>
        </section>
      </ResizableSidebarSections>
      <footer className="sidebar-footer" inert={modalVisible}>
        <button
          type="button"
          className={`settings-button${state?.settings_selected ? " is-selected" : ""}`}
          onClick={() => postNative({ type: "show_settings" })}
        >
          <span className="settings-icon" aria-hidden="true"><Settings size={15} /></span>
          <span>{copy.settings}</span>
        </button>
      </footer>
      {menu && <ContextMenu menu={menu} close={() => setMenu(null)} />}
      {modalVisible && <div className="sidebar-modal-backdrop" aria-hidden="true" />}
    </aside>
    </Now.Provider>
  );
}

const root = document.querySelector("#root");
if (!root) throw new Error("Blackholes navigation root was not found.");
createRoot(root).render(
  <StrictMode>
    <NavigationApp />
  </StrictMode>,
);

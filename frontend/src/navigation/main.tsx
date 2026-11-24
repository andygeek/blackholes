import { StrictMode, createContext, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { GroupIcon } from "../shared/GroupIcon";
import {
  ArrowDown,
  ArrowUp,
  ChevronRight,
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
import { SortableAgentList } from "./SortableAgentList";
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
  renameSection: string;
  deleteSection: string;
  moveSectionUp: string;
  moveSectionDown: string;
  emptySection: string;
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
}

/** Places `ids` in the slots their members occupy in `order`, as the app does when saving. */
function mergeOrder(order: string[], ids: string[]) {
  const requested = [...new Set(ids)];
  const wanted = new Set(requested);
  let next = 0;
  const merged = order.map(id => wanted.has(id) ? requested[next++] : id).filter((id): id is string => id !== undefined);
  return [...merged, ...requested.slice(next)];
}

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
  renameSection: "Renombrar sección",
  deleteSection: "Eliminar sección",
  moveSectionUp: "Subir sección",
  moveSectionDown: "Bajar sección",
  emptySection: "Sin sesiones",
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
  // Only pinned or sectioned sessions are listed while stopped; a click resumes them.
  const dormant = terminal.live === false && !terminal.needs_attention;
  const status = dormant ? copy.resumeSession : terminalStatus(terminal, copy);
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
    <div className={`agent-card terminal-agent-card${terminal.selected ? " is-selected" : ""}${terminal.needs_attention ? " needs-attention" : runsInBackground(terminal) ? " has-background" : ""}${terminal.attention_kind === "input-required" ? " is-awaiting-input" : ""}${terminal.pinned ? " is-pinned" : ""}${dormant ? " is-dormant" : ""}`}
      onContextMenu={event => {
        event.preventDefault();
        onMenu(new DOMRect(event.clientX, event.clientY, 0, 0), terminal);
      }}>
      <button type="button" className="agent-open"
        aria-current={terminal.selected ? "true" : undefined}
        aria-label={`${description}${status ? ` · ${status}` : ""}`}
        title={`${name ? `${name}\n` : ""}${place}\n${terminal.provider_label}${terminal.note ? `\n${terminal.note}` : ""}${status ? `\n${status}` : ""}`}
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
  return (
    <button
      type="button"
      className={`tree-leaf terminal-tree-leaf tree-indent-${indentation}${terminal.selected ? " is-selected" : ""}${terminal.attention_kind === "input-required" ? " is-awaiting-input" : ""}`}
      id={`nav-terminal-${terminal.id}`}
      aria-label={`${terminal.label}${status ? ` · ${status}` : ""}`}
      title={`${terminal.label}${status ? ` · ${status}` : ""}`}
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

function NavigationApp() {
  const [state, setState] = useState<NavigationState | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [modalVisible, setModalVisible] = useState(false);
  const [revealTarget, setRevealTarget] = useState<{ id: string } | null>(null);
  const revealTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const revealedRow = useRef<HTMLElement | null>(null);
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
    // Move only the Projects viewport, never the Agents list or the whole WebView.
    const viewport = row.closest<HTMLElement>(".sidebar-scroll");
    if (!viewport) return;
    const bounds = row.getBoundingClientRect();
    const viewportBounds = viewport.getBoundingClientRect();
    viewport.scrollTo({
      top: viewport.scrollTop + bounds.top - viewportBounds.top - (viewport.clientHeight - bounds.height) / 2,
      behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth",
    });
    row.classList.add("is-revealed");
    revealedRow.current = row;
    revealTimer.current = setTimeout(() => row.classList.remove("is-revealed"), 1800);
    setRevealTarget(null);
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
          setState(next);
        }
      },
    };
    postNative({ type: "ready" });
    return () => { delete window.blackholesNavigation; };
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
  const openAgentMenu = (anchor: DOMRect, terminal: TerminalItem) => {
    const current = sections.find(section => section.id === sectionOf(terminal));
    openMenu(anchor, [terminal.pinned
      ? { label: copy.unpin, icon: PinOff, command: { type: "set_terminal_pinned", terminal_id: terminal.id, pinned: false } }
      : { label: copy.pin, icon: Pin, command: { type: "set_terminal_pinned", terminal_id: terminal.id, pinned: true } },
      { label: copy.rename, icon: Pencil, command: { type: "rename_terminal", terminal_id: terminal.id } },
      ...sections.filter(section => section !== current).map(section => ({
        label: copy.moveToSection.replace("{name}", section.name), icon: FolderInput,
        command: { type: "set_terminal_section", terminal_id: terminal.id, section_id: section.id },
      })),
      ...(current ? [{ label: copy.removeFromSection.replace("{name}", current.name), icon: FolderMinus,
        command: { type: "set_terminal_section", terminal_id: terminal.id, section_id: null } }] : []),
      { label: copy.newSectionWithSession, icon: FolderPlus, command: { type: "new_agent_section", terminal_id: terminal.id } },
      { label: copy.closeTerminal, icon: X, danger: true, command: { type: "close_terminal", terminal_id: terminal.id } }]);
  };
  const openSectionMenu = (anchor: DOMRect, section: AgentSectionItem, index: number) => openMenu(anchor, [
    { label: copy.renameSection, icon: Pencil, command: { type: "rename_agent_section", section_id: section.id } },
    ...(index > 0 ? [{ label: copy.moveSectionUp, icon: ArrowUp, command: { type: "move_agent_section", section_id: section.id, offset: -1 } }] : []),
    ...(index < sections.length - 1 ? [{ label: copy.moveSectionDown, icon: ArrowDown, command: { type: "move_agent_section", section_id: section.id, offset: 1 } }] : []),
    { label: copy.deleteSection, icon: Trash2, danger: true, command: { type: "delete_agent_section", section_id: section.id } },
  ]);
  const setSectionFolded = (section: AgentSectionItem, collapsed: boolean) => {
    setMenu(null);
    setState(previous => previous ? { ...previous, agent_sections: (previous.agent_sections || []).map(current =>
      current.id === section.id ? { ...current, collapsed } : current) } : previous);
    postNative({ type: "set_agent_section_collapsed", section_id: section.id, collapsed });
  };
  /** Cards of one block (no section, or one section), sorted and reordered on their own. */
  const agentBlock = (key: string, terminals: TerminalItem[]) => {
    const members = new Set(terminals.map(terminal => terminal.id));
    // Pinned sessions stay on top in pin order; their notices still show in their own colors.
    const pinnedAgents = terminals.filter(terminal => terminal.pinned)
      .sort((a, b) => (a.pin_rank ?? Number.MAX_SAFE_INTEGER) - (b.pin_rank ?? Number.MAX_SAFE_INTEGER));
    const unpinnedOrder = manualAgentOrder.filter(terminal => !terminal.pinned && members.has(terminal.id));
    const unpinnedAgents = [...unpinnedOrder].sort((a, b) =>
      Number(b.needs_attention) - Number(a.needs_attention)
        || (a.needs_attention ? (a.attention_rank ?? Number.MAX_SAFE_INTEGER) - (b.attention_rank ?? Number.MAX_SAFE_INTEGER)
          : Number(b.working) - Number(a.working)));
    const items = [...pinnedAgents, ...unpinnedAgents].map(terminal => ({
      id: `terminal:${terminal.id}`, label: [sessionName(terminal), sessionPlace(terminal)].filter(Boolean).join(" · "),
      reorderable: terminal.pinned || !terminal.needs_attention,
      reorderGroup: terminal.pinned ? "pinned" : terminal.working ? "working" : "idle",
      statusLabel: terminal.attention_kind === "completed" ? copy.responseReady : copy.needsAttention,
      content: <TerminalAgentRow terminal={terminal} copy={copy} onOpen={openTerminalAgent} onMenu={openAgentMenu} />,
    }));
    const pinnedIds = pinnedAgents.map(terminal => `terminal:${terminal.id}`);
    return <SortableAgentList key={key} items={items} disabled={modalVisible} language={state?.language || "es"}
      onReorder={ids => {
        if (ids.some(id => pinnedIds.includes(id)) && ids.filter(id => pinnedIds.includes(id)).join() !== pinnedIds.join()) {
          const pinned = ids.filter(id => pinnedIds.includes(id)).map(id => id.replace(/^terminal:/, ""));
          setState(previous => {
            if (!previous) return previous;
            const order = mergeOrder(previous.terminal_agents.filter(terminal => terminal.pinned)
              .sort((a, b) => (a.pin_rank ?? Number.MAX_SAFE_INTEGER) - (b.pin_rank ?? Number.MAX_SAFE_INTEGER))
              .map(terminal => terminal.id), pinned);
            return { ...previous, terminal_agents: previous.terminal_agents.map(terminal => ({
              ...terminal, pin_rank: terminal.pinned ? order.indexOf(terminal.id) : terminal.pin_rank,
            })) };
          });
          postNative({ type: "reorder_pinned", ids: pinned });
          return;
        }
        ids = ids.filter(id => !pinnedIds.includes(id));
        const pending = new Set(unpinnedOrder.filter(terminal => terminal.needs_attention).map(terminal => `terminal:${terminal.id}`));
        const reordered = ids.filter(id => !pending.has(id));
        let index = 0;
        // Notification priority never overwrites the saved positions of pending agents.
        const order = unpinnedOrder.map(terminal =>
          pending.has(`terminal:${terminal.id}`) ? `terminal:${terminal.id}` : reordered[index++]);
        setState(previous => previous ? { ...previous, agent_order: mergeOrder(previous.agent_order || [], order) } : previous);
        postNative({ type: "reorder_agents", ids: order });
      }} />;
  };
  const agentTerminals = state?.terminal_agents || [];
  const unsectioned = agentTerminals.filter(terminal => !sectionOf(terminal));

  return (
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
          {agentTerminals.length === 0 && sections.length === 0 && <p className="agents-empty">{state?.language === "en"
            ? "No active agents. Open a saved session from Projects or start one with +."
            : "No hay agentes activos. Abre una sesión guardada desde Proyectos o inicia una con +."}</p>}
          {unsectioned.length > 0 && agentBlock("unsectioned", unsectioned)}
          {sections.map((section, index) => {
            const members = agentTerminals.filter(terminal => sectionOf(terminal) === section.id);
            const notice = section.collapsed ? pendingNotice(members) : null;
            const background = section.collapsed && !notice && anyInBackground(members);
            return <div key={section.id} className="agent-section">
              <div className="agent-section-header" onContextMenu={event => {
                event.preventDefault();
                openSectionMenu(new DOMRect(event.clientX, event.clientY, 0, 0), section, index);
              }}>
                <button type="button" className="agent-section-toggle" aria-expanded={!section.collapsed}
                  aria-label={`${section.name} · ${members.length}${notice ? ` · ${noticeLabel(notice, copy)}` : ""}`}
                  disabled={modalVisible} onClick={() => setSectionFolded(section, !section.collapsed)}>
                  <ChevronRight size={13} aria-hidden="true" />
                  <span className="agent-section-name">{section.name}</span>
                  <span className="agent-section-count" aria-hidden="true">{members.length}</span>
                  {(notice || background) && <span aria-hidden="true" className={`navigation-status-dot agents-summary-dot${notice === "input-required" ? " is-awaiting-input" : background ? " has-background" : ""}`} />}
                </button>
                <button type="button" className="row-action agent-section-options" aria-label={`${copy.options}: ${section.name}`} title={copy.options}
                  disabled={modalVisible} onClick={event => {
                    event.stopPropagation();
                    openSectionMenu(event.currentTarget.getBoundingClientRect(), section, index);
                  }}>
                  <MoreHorizontal size={14} />
                </button>
              </div>
              {!section.collapsed && (members.length
                ? agentBlock(section.id, members)
                : <p className="agent-section-empty">{copy.emptySection}</p>)}
            </div>;
          })}
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
  );
}

const root = document.querySelector("#root");
if (!root) throw new Error("Blackholes navigation root was not found.");
createRoot(root).render(
  <StrictMode>
    <NavigationApp />
  </StrictMode>,
);

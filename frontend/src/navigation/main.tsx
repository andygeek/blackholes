import { StrictMode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  Bot,
  ChevronRight,
  CircleDot,
  Code2,
  Database,
  FilePenLine,
  Folder,
  Gem,
  GitBranch,
  Globe2,
  Layers3,
  ListTodo,
  MoreHorizontal,
  NotebookPen,
  PanelLeftClose,
  Plus,
  RefreshCw,
  Rocket,
  Settings,
  Sparkles,
  SquareTerminal,
  Trash2,
  X,
  type LucideIcon,
} from "lucide-react";
import { AgentAvatar } from "../shared/AgentAvatar";
import { SidebarResizeHandle } from "../shared/SidebarResizeHandle";
import { SidebarScrollArea } from "./SidebarScrollArea";
import { postNative, type NativeCommand } from "../shared/native";
import { applyAppTheme, type AppTheme } from "../shared/theme";

interface Copy {
  projects: string;
  project: string;
  settings: string;
  working: string;
  terminal: string;
  tasks: string;
  task: string;
  notes: string;
  new: string;
  toggle: string;
  options: string;
  removeAgent: string;
  closeTerminal: string;
  newTerminal: string;
  newTask: string;
  addAgent: string;
  assignAgent: string;
  refreshProject: string;
  addToProject: string;
  cloneLocalRepository: string;
  cloneGithubRepository: string;
  editProject: string;
  projectSettings: string;
  removeProject: string;
  editTask: string;
  removeTask: string;
}

interface AgentItem {
  scope: string;
  name: string;
  preview: string;
  selected: boolean;
  busy: boolean;
  identity: string;
  removable: boolean;
  arriving?: boolean;
  context?: {
    kind: "project" | "task";
    label: string;
  } | null;
}

interface TerminalItem {
  id: string;
  label: string;
  agent: string;
  busy: boolean;
  selected: boolean;
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
  notes_selected: boolean;
  agents?: AgentItem[];
  agent?: AgentItem;
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
  notes_selected: boolean;
  agents?: AgentItem[];
  agent?: AgentItem;
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
  global_agents: AgentItem[];
  projects: ProjectItem[];
}

interface MenuItem {
  label: string;
  icon: LucideIcon;
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
  terminal: "Terminal",
  tasks: "Tareas",
  task: "Tarea",
  notes: "Notas",
  new: "Nuevo",
  toggle: "Expandir o contraer",
  options: "Opciones",
  removeAgent: "Eliminar Black Bot",
  closeTerminal: "Cerrar terminal",
  newTerminal: "Nueva terminal",
  newTask: "Agregar tarea",
  addAgent: "Agregar bot",
  assignAgent: "Asignar Black Bot",
  refreshProject: "Buscar repositorios nuevos",
  addToProject: "Agregar al proyecto",
  cloneLocalRepository: "Agregar repositorio local…",
  cloneGithubRepository: "Agregar repositorio de GitHub…",
  editProject: "Editar proyecto",
  projectSettings: "Configuración del proyecto",
  removeProject: "Eliminar proyecto",
  editTask: "Editar tarea",
  removeTask: "Eliminar tarea",
};

const iconFor = (value?: string): LucideIcon => ({
  folder: Folder,
  layers: Layers3,
  globe: Globe2,
  rocket: Rocket,
  code: Code2,
  "code-2": Code2,
  "list-todo": ListTodo,
  database: Database,
  terminal: SquareTerminal,
  "square-terminal": SquareTerminal,
  branch: GitBranch,
  "git-branch": GitBranch,
  notes: NotebookPen,
  claude: Sparkles,
  codex: CircleDot,
  gemini: Gem,
  shell: SquareTerminal,
}[value || ""] || Layers3);

function WorkingDots({ label }: { label: string }) {
  return (
    <span className="working-dots" aria-label={label}>
      <i /><i /><i />
    </span>
  );
}

function Avatar({ agent, size }: { agent: AgentItem; size: number }) {
  return (
    <span className="agent-avatar-wrap">
      <AgentAvatar identity={agent.identity} size={size} busy={agent.busy} />
      {agent.busy && <span className="active-badge" />}
    </span>
  );
}

function AgentName({ agent, copy, showContext = false }: { agent: AgentItem; copy: Copy; showContext?: boolean }) {
  return (
    <span className="agent-name-line">
      <span className="agent-name">{agent.name}</span>
      {showContext && agent.context && (
        <span className={`agent-context-chip is-${agent.context.kind}`} title={`${agent.context.kind === "task" ? copy.task : copy.project}: ${agent.context.label}`}>
          {agent.context.kind === "task" ? <ListTodo size={10} aria-hidden="true" /> : <Folder size={10} aria-hidden="true" />}
          <span>{agent.context.label}</span>
        </span>
      )}
      {agent.busy && <WorkingDots label={copy.working} />}
    </span>
  );
}

function GlobalAgentRow({ agent, copy }: { agent: AgentItem; copy: Copy }) {
  return (
    <div
      className={`agent-card${agent.selected ? " is-selected" : ""}${agent.arriving ? " is-arriving" : ""}`}
    >
      <button type="button" className="agent-open" aria-current={agent.selected ? "true" : undefined}
        aria-label={[agent.name, agent.context?.label].filter(Boolean).join(" · ")}
        onClick={() => postNative({ type: "open_agent", scope: agent.scope })}>
        <Avatar agent={agent} size={32} />
        <span className="agent-copy">
          <AgentName agent={agent} copy={copy} showContext />
          <span className="agent-preview">{agent.preview}</span>
        </span>
      </button>
      {agent.removable && (
        <button
          type="button"
          className="remove-button"
          aria-label={copy.removeAgent}
          onClick={(event) => {
            event.stopPropagation();
            postNative({ type: "remove_agent", scope: agent.scope });
          }}
        >
          <X size={14} />
        </button>
      )}
    </div>
  );
}

function TreeAgent({ agent, copy, indentation }: { agent: AgentItem; copy: Copy; indentation: number }) {
  return (
    <button
      type="button"
      className={`tree-agent tree-indent-${indentation}${agent.selected ? " is-selected" : ""}`}
      aria-label={agent.name}
      onClick={() => postNative({ type: "open_agent", scope: agent.scope })}
    >
      <Avatar agent={agent} size={18} />
      <span className="tree-label"><AgentName agent={agent} copy={copy} /></span>
      <button
        type="button"
        className="remove-button"
        aria-label={copy.removeAgent}
        onClick={(event) => {
          event.stopPropagation();
          postNative({ type: "remove_agent", scope: agent.scope });
        }}
      >
        <X size={13} />
      </button>
    </button>
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
  const Icon = iconFor(terminal.agent);
  return (
    <button
      type="button"
      className={`tree-leaf tree-indent-${indentation}${terminal.selected ? " is-selected" : ""}`}
      aria-label={terminal.label}
      onClick={() => postNative({ type: "focus_terminal", terminal_id: terminal.id })}
    >
      <span className="agent-kind-icon-wrap">
        <span className="agent-kind-icon"><Icon size={13} /></span>
        {terminal.busy && <span className="active-badge" />}
      </span>
      <span className="tree-label">
        <span className="agent-name-line">
          <span className="tree-label">{terminal.label}</span>
          {terminal.busy && <WorkingDots label={copy.working} />}
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

function NotesRow({ workspaceId, taskId, selected, copy, indentation }: {
  workspaceId: string;
  taskId?: string;
  selected: boolean;
  copy: Copy;
  indentation: number;
}) {
  return (
    <button
      type="button"
      className={`tree-leaf tree-indent-${indentation}${selected ? " is-selected" : ""}`}
      aria-label={copy.notes}
      onClick={() => postNative(taskId
        ? { type: "task_notes", workspace_id: workspaceId, task_id: taskId }
        : { type: "project_notes", workspace_id: workspaceId })}
    >
      <span className="row-icon"><NotebookPen size={13} /></span>
      <span className="tree-label">{copy.notes}</span>
    </button>
  );
}

function RepositoryRow({ repository, workspaceId, taskId, copy, indentation, openMenu }: {
  repository: RepositoryItem;
  workspaceId: string;
  taskId?: string;
  copy: Copy;
  indentation: number;
  openMenu(anchor: DOMRect, items: MenuItem[]): void;
}) {
  const launchItems = (): MenuItem[] => [
    {
      label: copy.addAgent,
      icon: Bot,
      command: { type: "create_scoped_agent", workspace_id: workspaceId, task_id: taskId || null },
    },
    {
      label: copy.terminal,
      icon: SquareTerminal,
      command: {
        type: "new_terminal",
        workspace_id: workspaceId,
        task_id: taskId || null,
        repository_id: repository.id,
        agent: "shell",
      },
    },
  ];
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
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), launchItems())}
          />
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
  const agents = task.agents || (task.agent ? [task.agent] : []);
  return (
    <>
      <button
        type="button"
        className={`tree-row tree-indent-1${task.selected ? " is-selected" : ""}`}
        id={`nav-task-${task.id}`}
        style={{ "--row-color": task.color } as React.CSSProperties}
        aria-label={task.title}
        onClick={() => postNative({ type: "select_task", workspace_id: workspaceId, task_id: task.id })}
      >
        <Chevron expanded={task.expanded} copy={copy} command={{ type: "toggle_task", task_id: task.id }} />
        <span className="row-icon">{(() => { const Icon = iconFor(task.icon); return <Icon size={13} />; })()}</span>
        <span className="tree-label">{task.title}</span>
        {task.unseen && <span className="new-badge">{copy.new}</span>}
        <span className="row-actions">
          <RowAction
            label={copy.newTerminal}
            icon={Plus}
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), [
              {
                label: copy.addAgent,
                icon: Bot,
                command: { type: "create_scoped_agent", workspace_id: workspaceId, task_id: task.id },
              },
              {
                label: copy.terminal,
                icon: SquareTerminal,
                command: { type: "new_terminal", workspace_id: workspaceId, task_id: task.id, repository_id: null, agent: "shell" },
              },
            ])}
          />
          <RowAction
            label={copy.options}
            icon={MoreHorizontal}
            onClick={(event) => {
              const items: MenuItem[] = [
                { label: copy.editTask, icon: FilePenLine, command: { type: "edit_task", task_id: task.id } },
                { label: copy.removeTask, icon: Trash2, danger: true, command: { type: "remove_task", task_id: task.id } },
              ];
              if (agents.length === 0) {
                items.splice(1, 0, { label: copy.assignAgent, icon: Bot, command: { type: "assign_task_agent", task_id: task.id } });
              }
              openMenu(event.currentTarget.getBoundingClientRect(), items);
            }}
          />
        </span>
      </button>
      {task.expanded && (
        <>
          {agents.map((agent) => <TreeAgent key={agent.scope} agent={agent} copy={copy} indentation={2} />)}
          <NotesRow workspaceId={workspaceId} taskId={task.id} selected={task.notes_selected} copy={copy} indentation={2} />
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
  const agents = project.agents || (project.agent ? [project.agent] : []);
  const Icon = iconFor(project.icon);
  return (
    <section className="project-block">
      <button
        type="button"
        className={`tree-row${project.selected ? " is-selected" : ""}`}
        id={`nav-project-${project.id}`}
        style={{ "--row-color": project.color } as React.CSSProperties}
        aria-label={project.label}
        onClick={() => postNative({ type: "select_project", workspace_id: project.id })}
      >
        <Chevron expanded={project.expanded} copy={copy} command={{ type: "toggle_project", workspace_id: project.id }} />
        <span className="row-icon"><Icon size={13} /></span>
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
              { label: copy.addAgent, icon: Bot, command: { type: "create_scoped_agent", workspace_id: project.id, task_id: null } },
              { label: copy.cloneLocalRepository, icon: Plus, command: { type: "add_project_repository", workspace_id: project.id, github: false } },
              { label: copy.cloneGithubRepository, icon: Plus, command: { type: "add_project_repository", workspace_id: project.id, github: true } },
              { label: copy.newTerminal, icon: SquareTerminal, command: { type: "new_terminal", workspace_id: project.id, task_id: null, repository_id: null, agent: "shell" } },
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
          {agents.map((agent) => <TreeAgent key={agent.scope} agent={agent} copy={copy} indentation={1} />)}
          <NotesRow workspaceId={project.id} selected={project.notes_selected} copy={copy} indentation={1} />
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
            <span className="menu-item__icon"><Icon size={14} /></span>
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

  useLayoutEffect(() => {
    if (!revealTarget) return;
    const row = document.getElementById(revealTarget.id);
    if (!row) return; // A hydrate may still be expanding the parent sections.
    if (revealTimer.current) clearTimeout(revealTimer.current);
    revealedRow.current?.classList.remove("is-revealed");
    row.scrollIntoView({ block: "center", behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth" });
    row.classList.add("is-revealed");
    revealedRow.current = row;
    revealTimer.current = setTimeout(() => row.classList.remove("is-revealed"), 1800);
    setRevealTarget(null);
  }, [state, revealTarget]);

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

  return (
    <aside className="sidebar" aria-label="Blackholes">
      <SidebarResizeHandle width={state?.sidebar_width ?? 280} right={-1} hitWidth={4} label={state?.language === "en" ? "Resize sidebar" : "Cambiar ancho del menú lateral"} />
      <header className="brand">
        <button className="brand__name" type="button" onClick={() => postNative({ type: "open_agent", scope: "global" })}>
          BLACKHOLES
        </button>
      </header>
      <SidebarScrollArea label={state?.language === "en" ? "Scroll projects and agents" : "Desplazar proyectos y agentes"}>
        <section className="agents-shell" aria-label={state?.language === "en" ? "Agents" : "Agentes"}>
          <header className="section-header">
            <span>{state?.language === "en" ? "Agents" : "Agentes"}</span>
            <span className="section-header__actions">
              <button className="icon-button" type="button" aria-label={copy.addAgent} title={copy.addAgent} onClick={() => postNative({ type: "create_global_agent" })}>
                <Plus size={17} />
              </button>
            </span>
          </header>
          <div className="global-agents">
            {(state?.global_agents || []).map((agent) => (
              <GlobalAgentRow key={agent.scope} agent={agent} copy={copy} />
            ))}
          </div>
        </section>
        <section className="projects-shell">
          <header className="section-header">
            <span>{copy.projects}</span>
            <span className="section-header__actions">
              <button className="icon-button" type="button" aria-label="Contraer todo" title="Contraer todo" onClick={() => postNative({ type: "collapse_all" })}>
                <PanelLeftClose size={15} />
              </button>
              <button className="icon-button" type="button" aria-label="Nuevo proyecto" title="Nuevo proyecto" onClick={() => postNative({ type: "new_project" })}>
                <Plus size={17} />
              </button>
            </span>
          </header>
          <nav className="projects" aria-label={copy.projects}>
            {(state?.projects || []).map((project) => (
              <ProjectBlock
                key={project.id}
                project={project}
                copy={copy}
                openMenu={openMenu}
              />
            ))}
          </nav>
        </section>
      </SidebarScrollArea>
      <footer className="sidebar-footer">
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

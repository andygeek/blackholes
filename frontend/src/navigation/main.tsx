import { StrictMode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { GroupIcon } from "../shared/GroupIcon";
import {
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
  ChevronsUp,
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
  needs_attention: boolean;
  attention_kind: NoticeKind | null;
  attention_rank: number | null;
  working: boolean;
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
  agent_order?: string[];
  projects: ProjectItem[];
}

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
  needsAttention: "Necesita tu atención",
  responseReady: "Respuesta lista",
  terminal: "Terminal",
  blankTerminal: "Terminal vacía",
  tasks: "Grupos de worktrees",
  task: "Grupo de worktrees",
  new: "Nuevo",
  toggle: "Expandir o contraer",
  options: "Opciones",
  closeTerminal: "Cerrar terminal",
  newTerminal: "Nueva terminal",
  newTask: "Agregar grupo de worktrees",
  refreshProject: "Buscar repositorios nuevos",
  addToProject: "Agregar al proyecto",
  addRepository: "Agregar repositorio…",
  removeRepository: "Eliminar repositorio",
  editProject: "Editar proyecto",
  projectSettings: "Configuración del proyecto",
  removeProject: "Eliminar proyecto",
  editTask: "Editar grupo de worktrees",
  removeTask: "Cerrar grupo",
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
  claude: Sparkles,
  codex: CircleDot,
  gemini: Gem,
  shell: SquareTerminal,
}[value || ""] || Layers3);

const terminalNotice = (terminal: TerminalItem): NoticeKind | null => terminal.needs_attention
  ? terminal.attention_kind ?? "input-required" : null;

const noticeLabel = (notice: NoticeKind | null, copy: Copy) => notice === "completed"
  ? copy.responseReady : notice === "input-required" ? copy.needsAttention : "";

const terminalStatus = (terminal: TerminalItem, copy: Copy) => noticeLabel(terminalNotice(terminal), copy)
  || (terminal.working ? copy.working : "");

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

function TerminalAgentRow({ terminal, copy, onOpen }: { terminal: TerminalItem; copy: Copy; onOpen(terminal: TerminalItem): void }) {
  const status = terminalStatus(terminal, copy);
  const preview = terminal.label === terminal.provider_label
    ? terminal.provider_label
    : `${terminal.provider_label} · ${terminal.label}`;
  return (
    <div className={`agent-card terminal-agent-card${terminal.selected ? " is-selected" : ""}${terminal.needs_attention ? " needs-attention" : ""}${terminal.attention_kind === "input-required" ? " is-awaiting-input" : ""}`}>
      <button type="button" className="agent-open"
        aria-current={terminal.selected ? "true" : undefined}
        aria-label={`${terminal.context} · ${preview}${status ? ` · ${status}` : ""}`}
        title={`${terminal.context}\n${preview}${status ? `\n${status}` : ""}`}
        onClick={() => onOpen(terminal)}>
        <NavigationStatusIcon notice={terminalNotice(terminal)} working={terminal.working} large>
          <TerminalProviderIcon provider={terminal.agent} />
        </NavigationStatusIcon>
        <span className="agent-copy">
          <span className="agent-name-line"><span className="agent-name">
            {terminal.group_title ? <>
              {terminal.project_label}/<span className="agent-group-icon" style={{ color: terminal.group_color || undefined }}>
                <GroupIcon value={terminal.group_icon || "layers"} size={12} />
              </span>{terminal.group_title}
            </> : terminal.context}
          </span></span>
          <span className="agent-preview">{preview}</span>
        </span>
      </button>
      <button type="button" className="remove-button"
        aria-label={`${copy.closeTerminal}: ${terminal.context} · ${preview}`}
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
      <NavigationStatusIcon notice={terminalNotice(terminal)} working={terminal.working}>
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

function terminalLaunchItems(workspaceId: string, taskId: string | null | undefined, repositoryId: string | null, copy: Copy): MenuItem[] {
  return [
    ["shell", copy.blankTerminal], ["claude", "Claude"], ["codex", "Codex"],
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
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), terminalLaunchItems(workspaceId, taskId, repository.id, copy))}
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
  const notice = pendingNotice(taskTerminals(task));
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
        <NavigationStatusIcon notice={notice}><GroupIcon value={task.icon} size={13} /></NavigationStatusIcon>
        <span className="tree-label">{task.title}</span>
        {task.unseen && <span className="new-badge">{copy.new}</span>}
        <span className="row-actions">
          <RowAction
            label={copy.newTerminal}
            icon={Plus}
            onClick={(event) => openMenu(event.currentTarget.getBoundingClientRect(), terminalLaunchItems(workspaceId, task.id, null, copy))}
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
  const Icon = iconFor(project.icon);
  const notice = pendingNotice([
    ...project.terminals,
    ...project.repositories.flatMap(repository => repository.terminals),
    ...project.tasks.flatMap(taskTerminals),
  ]);
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
        <NavigationStatusIcon notice={notice}><Icon size={13} /></NavigationStatusIcon>
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
              ...terminalLaunchItems(project.id, null, null, copy),
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
  const agentItems = [...manualAgentOrder].sort((a, b) =>
    Number(b.needs_attention) - Number(a.needs_attention)
      || (a.needs_attention ? (a.attention_rank ?? Number.MAX_SAFE_INTEGER) - (b.attention_rank ?? Number.MAX_SAFE_INTEGER) : 0)
  ).map(terminal => ({
    id: `terminal:${terminal.id}`, label: `${terminal.context} · ${terminal.provider_label}`,
    reorderable: !terminal.needs_attention,
    statusLabel: terminal.attention_kind === "completed" ? copy.responseReady : copy.needsAttention,
    content: <TerminalAgentRow terminal={terminal} copy={copy} onOpen={openTerminalAgent} />,
  }));

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
              {agentsNotice && <span aria-hidden="true" className={`navigation-status-dot agents-summary-dot${agentsNotice === "input-required" ? " is-awaiting-input" : ""}`} />}
            </button>
          </header>
          <SidebarScrollArea id="sidebar-agents-content" hidden={agentsCollapsed}
            label={state?.language === "en" ? "Scroll agents" : "Desplazar agentes"}>
          {agentItems.length === 0 && <p className="agents-empty">{state?.language === "en"
            ? "No active agents. Open a saved session from Projects or start one with +."
            : "No hay agentes activos. Abre una sesión guardada desde Proyectos o inicia una con +."}</p>}
          <SortableAgentList items={agentItems} disabled={modalVisible} language={state?.language || "es"}
            onReorder={ids => {
              const pending = new Set(manualAgentOrder.filter(terminal => terminal.needs_attention).map(terminal => `terminal:${terminal.id}`));
              const reordered = ids.filter(id => !pending.has(id));
              let index = 0;
              // Notification priority never overwrites the saved positions of pending agents.
              const order = manualAgentOrder.map(terminal =>
                pending.has(`terminal:${terminal.id}`) ? `terminal:${terminal.id}` : reordered[index++]);
              setState(previous => previous ? { ...previous, agent_order: order } : previous);
              postNative({ type: "reorder_agents", ids: order });
            }} />
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
              <button className="icon-button" type="button" aria-label={state?.language === "en" ? "Collapse all projects and worktree groups" : "Contraer todos los proyectos y grupos de worktrees"} title={state?.language === "en" ? "Collapse all projects and worktree groups" : "Contraer todos los proyectos y grupos de worktrees"} onClick={() => postNative({ type: "collapse_all" })}>
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
            {(state?.projects || []).map((project) => (
              <ProjectBlock
                key={project.id}
                project={project}
                copy={copy}
                openMenu={openMenu}
              />
            ))}
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

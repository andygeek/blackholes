import type { AgentIdentity } from "../shared/AgentAvatar";
import type { AppTheme } from "../shared/theme";

export interface ChatAttachment {
  id: string;
  media_type: string;
  data: string;
}

export interface ChatActivity {
  agent?: string;
  tool?: string;
  detail?: string | null;
  created_at?: string;
  task_id?: string | null;
  status?: "running" | "foreground" | "completed" | "failed" | "stopped" | "blocked" | string | null;
  summary?: string | null;
  background?: boolean;
}

export interface ChatHandoff {
  scope: string;
  project_id?: string | null;
  task_id?: string | null;
  label?: string;
  identity?: AgentIdentity | string;
  navigation?: boolean;
}

export interface BranchNavigation {
  total: number;
  position: number;
  previous_branch_id?: string | null;
  next_branch_id?: string | null;
}

export interface ChatMessage {
  duration_ms?: number | null;
  id: string;
  role: "user" | "assistant";
  content: string;
  created_at: string;
  attachments: ChatAttachment[];
  activities: ChatActivity[];
  handoffs: ChatHandoff[];
  branch_navigation?: BranchNavigation | null;
  interrupted?: boolean;
  interruptedLabel?: string;
  streaming?: boolean;
  activityLabel?: string;
  error?: boolean;
}

export interface ActiveResponse {
  created_at?: string | null;
  id: string;
  after_id?: string;
  text?: string;
  activities?: ChatActivity[];
  handoffs?: ChatHandoff[];
}

export interface ChatModelOption {
  disabled?: boolean;
  value: string;
  label: string;
}

export interface AppModalState {
  kind: "remove_project" | "remove_agent" | "remove_task" | "create_project" | "create_task";
  task_id?: string;
  request_id?: string;
  projects_root?: string;
  feedback?: { path?: string | null; error?: string; branches?: TaskBranchAvailability[] };
  repositories?: { id: string; name: string }[];
  workspace_id?: string;
  scope?: string;
  title: string;
  name: string;
  context?: string;
  description: string;
  confirm_label: string;
  cancel_label: string;
  offset_x?: number;
}

export interface TaskBranchAvailability {
  repositoryId: string;
  repositoryName: string;
  localRevision?: string | null;
  remoteRevision?: string | null;
  localCheckedOut: boolean;
  base?: { label: string; revision: string } | null;
}

export interface HydrateEvent {
  type: "hydrate";
  sidebar_width?: number;
  language?: "en" | "es";
  theme?: AppTheme;
  agent_name?: string;
  agent_identity?: AgentIdentity | string;
  context_label?: string;
  agent_context?: { kind: "project" | "task"; label: string; project_label: string } | null;
  placeholder?: string;
  welcome?: string;
  messages?: ChatMessage[];
  busy?: boolean;
  active_response?: ActiveResponse | null;
  full_access?: boolean;
  permission_control_supported?: boolean;
  provider_label?: string;
  model?: string;
  model_label?: string;
  model_options?: ChatModelOption[];
  model_catalog_loading?: boolean;
  model_catalog_error?: boolean;
  model_control_supported?: boolean;
}

export type ChatNativeEvent = HydrateEvent | {
  sidebar_width?: number;
  duration_ms?: number | null;
  type: string;
  id?: string;
  after_id?: string;
  response_id?: string;
  text?: string;
  message?: string;
  created_at?: string;
  status?: string;
  fallback?: string;
  label?: string;
  activity?: ChatActivity;
  handoff?: ChatHandoff;
  attachment?: ChatAttachment;
  surface?: string;
  data?: unknown;
  error?: boolean;
  theme?: AppTheme;
  full_access?: boolean;
  permission_control_supported?: boolean;
  provider_label?: string;
  model?: string;
  model_label?: string;
  model_control_supported?: boolean;
  modal?: AppModalState | null;
  model_options?: ChatModelOption[];
  model_catalog_loading?: boolean;
  model_catalog_error?: boolean;
  request_id?: string;
  feedback?: AppModalState["feedback"];
};

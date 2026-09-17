export interface AppModalState {
  kind: "remove_project" | "close_terminal" | "remove_task" | "create_project" | "edit_project" | "create_task" | "add_repository" | "remove_repository";
  icon?: string;
  color_id?: string;
  icon_options?: { value: string; label: string }[];
  color_options?: { value: string; label: string; color: string }[];
  terminal_id?: string;
  repository_id?: string;
  task_id?: string;
  request_id?: string;
  projects_root?: string;
  over_terminal?: boolean;
  feedback?: { path?: string | null; error?: string; completed_sources?: string[]; branches?: TaskBranchAvailability[]; repositories?: { name: string; path: string }[] };
  repositories?: { id: string; name: string }[];
  workspace_id?: string;
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

import { createId, postNative } from "../shared/native";

export type GroupLink = { url: string; label: string | null; kind: "pull_request" | "context" };
export type TaskMetadata = { title: string; icon: string; description: string | null; links: GroupLink[] };
type Snapshot = { values: TaskMetadata; revision: string };
export type GroupDraft = Snapshot & {
  base: TaskMetadata;
  seenRevision: string;
  pending?: { id: string; values: TaskMetadata };
  remote?: Snapshot;
  error?: string;
  timer?: number;
};
const drafts = new Map<string, GroupDraft>();
const keys = ["title", "icon", "description", "links"] as const;
export const copyDetails = (value: TaskMetadata): TaskMetadata => ({ title: value.title, icon: value.icon || "layers", description: value.description, links: value.links.map(link => ({ ...link })) });
const equal = (a: unknown, b: unknown) => JSON.stringify(a ?? "") === JSON.stringify(b ?? "");
export const draftChanged = (draft: GroupDraft) => keys.some(key => !equal(draft.values[key], draft.base[key]));
const notify = () => window.dispatchEvent(new Event("blackholes:task-details-saved"));
const markDirty = (id: string, draft: GroupDraft) => postNative({ type: "task_details_dirty", task_id: id, dirty: draftChanged(draft) || Boolean(draft.pending) });

export function getGroupDraft(id: string, values: TaskMetadata, revision: string): GroupDraft {
  let draft = drafts.get(id);
  if (!draft) { draft = { values: copyDetails(values), base: copyDetails(values), revision, seenRevision: revision }; drafts.set(id, draft); }
  return draft;
}

export function receiveGroupSnapshot(draft: GroupDraft, values: TaskMetadata, revision: string): boolean {
  // An old React prop snapshot is not a conflict with a newer save acknowledgement.
  if (revision === draft.seenRevision) return false;
  draft.seenRevision = revision;
  if (revision === draft.revision) { draft.remote = undefined; return true; }
  if (draftChanged(draft) || draft.pending) draft.remote = { values: copyDetails(values), revision };
  else { draft.values = copyDetails(values); draft.base = copyDetails(values); draft.revision = revision; draft.remote = undefined; }
  return true;
}

export function incompleteDraft(draft: GroupDraft): "title" | "links" | undefined {
  if (!draft.values.title.trim()) return "title";
  if (draft.values.links.some(link => {
    try {
      const url = new URL(link.url);
      return !/^https?:$/.test(url.protocol) || !url.hostname || Boolean(url.username || url.password) || /[\u0000-\u001f\u007f]/.test(link.url);
    } catch { return true; }
  })) return "links";
}

export function saveGroupDraft(id: string) {
  const draft = drafts.get(id);
  if (!draft) return;
  window.clearTimeout(draft.timer); draft.timer = undefined;
  if (!draftChanged(draft) || draft.pending || draft.remote || draft.error || incompleteDraft(draft)) return;
  const values = copyDetails(draft.values);
  const patch: Record<string, unknown> = Object.fromEntries(keys.filter(key => !equal(values[key], draft.base[key])).map(key => [key, values[key] || null]));
  // The document includes previous criteria; a rewrite replaces that slot too.
  if ("description" in patch) patch.acceptanceCriteria = null;
  draft.pending = { id: createId(), values };
  markDirty(id, draft);
  postNative({ type: "save_task_details", task_id: id, request_id: draft.pending.id, patch: { ...patch, expectedRevision: draft.revision } });
  notify();
}

function scheduleSave(id: string, draft: GroupDraft) {
  window.clearTimeout(draft.timer);
  draft.timer = window.setTimeout(() => saveGroupDraft(id), 900);
}

export function editGroupDraft(id: string, draft: GroupDraft, values: Partial<TaskMetadata>) {
  draft.values = { ...draft.values, ...values }; draft.error = undefined;
  markDirty(id, draft); scheduleSave(id, draft);
}

export function retryGroupSave(id: string, draft: GroupDraft) { draft.error = undefined; saveGroupDraft(id); notify(); }

export function reloadGroupDraft(id: string, draft: GroupDraft, values: TaskMetadata, revision: string) {
  window.clearTimeout(draft.timer);
  const latest = draft.remote || { values, revision };
  draft.values = copyDetails(latest.values); draft.base = copyDetails(latest.values); draft.revision = latest.revision;
  draft.remote = undefined; draft.error = undefined;
  markDirty(id, draft); notify();
}

// Acknowledgements are handled even when the user has navigated to another page.
// Edits made during a save survive and are submitted using the returned revision.
export function receiveTaskDetailsSave(event: { task_id: string; request_id: string; error?: string; details?: TaskMetadata; revision?: string }) {
  const draft = drafts.get(event.task_id);
  if (!draft || draft.pending?.id !== event.request_id) return;
  const sent = draft.pending.values;
  draft.pending = undefined;
  draft.error = event.error;
  if (!event.error && event.details && event.revision) {
    const acknowledged = copyDetails(event.details);
    const values = { ...draft.values };
    for (const key of keys) {
      if (equal(draft.values[key], sent[key])) {
        // Keep the caret and trailing blank lines while typing. The backend
        // trims optional text; that normalization is already saved.
        if (key === "description") acknowledged.description = draft.values.description;
        else if (key === "title") acknowledged.title = draft.values.title;
        else if (key === "icon") values.icon = acknowledged.icon;
        else values.links = acknowledged.links;
      }
    }
    draft.values = values; draft.base = acknowledged; draft.revision = event.revision;
    if (draft.remote?.revision === event.revision) draft.remote = undefined;
    if (draftChanged(draft)) scheduleSave(event.task_id, draft);
  } else if (!event.error) draft.error = "The save was not acknowledged. Retry to preserve this draft.";
  markDirty(event.task_id, draft); notify();
}

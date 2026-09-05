import { StrictMode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { ArrowUp, Check, ChevronDown, Plus, ShieldCheck, Square, SquarePen, X, Zap } from "lucide-react";
import { AgentAvatar, agentIdentityName, normalizeIdentity, type AgentIdentity } from "../shared/AgentAvatar";
import { createId, postNative } from "../shared/native";
import { applyAppTheme } from "../shared/theme";
import { SidebarResizeHandle } from "../shared/SidebarResizeHandle";
import { MessageView } from "./MessageView";
import { AppModal } from "./AppModal";
import { QuickOpen, type QuickOpenState } from "./QuickOpen";
import type { AppModalState, ChatAttachment, ChatHandoff, ChatMessage, ChatModelOption, ChatNativeEvent, HydrateEvent } from "./types";
import { WorkspaceSurface, type WorkspaceSurfaceEvent } from "./WorkspaceSurface";

const maxImages = 4;
const validImageTypes = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const invalidNavigationCharacter = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\uE000-\uF8FF\uFFFD]/;
const invalidNavigationCharacters = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\uE000-\uF8FF\uFFFD]/g;

const normalizeMessage = (message: Partial<ChatMessage> & Pick<ChatMessage, "id" | "role">): ChatMessage => ({
  id: message.id,
  role: message.role,
  content: message.content || "",
  created_at: message.created_at || new Date().toISOString(),
  duration_ms: message.duration_ms,
  attachments: message.attachments || [],
  activities: message.activities || [],
  handoffs: message.handoffs || [],
  branch_navigation: message.branch_navigation || null,
  interrupted: Boolean(message.interrupted),
  interruptedLabel: message.interruptedLabel,
  streaming: Boolean(message.streaming),
  activityLabel: message.activityLabel || "",
  error: Boolean(message.error),
});

const upsertMessage = (
  messages: ChatMessage[],
  id: string,
  update: (message: ChatMessage) => ChatMessage,
): ChatMessage[] => {
  const index = messages.findIndex((message) => message.id === id);
  const current = index >= 0
    ? messages[index]
    : normalizeMessage({ id, role: "assistant", streaming: true });
  const next = update(current);
  if (index < 0) return [...messages, next];
  return messages.map((message, messageIndex) => messageIndex === index ? next : message);
};

const handoffKey = (handoff: ChatHandoff): string => (
  `${handoff.navigation ? "navigation" : "handoff"}:${handoff.scope}:${handoff.task_id || handoff.project_id || ""}`
);

const attachmentDataUrl = (attachment: ChatAttachment): string => (
  `data:${attachment.media_type};base64,${attachment.data}`
);

const atBottom = (element: HTMLElement | null): boolean => (
  !element || element.scrollHeight - element.scrollTop - element.clientHeight < 90
);

const formatDay = (iso: string, language: "en" | "es"): string => {
  const date = new Date(iso);
  const today = new Date();
  if (date.toDateString() === today.toDateString()) return language === "en" ? "Today" : "Hoy";
  return new Intl.DateTimeFormat(language, {
    weekday: "short",
    day: "numeric",
    month: "short",
  }).format(date);
};

const arrowKey = (key: string, keyCode = 0): "up" | "down" | "left" | "right" | null => ({
  ArrowUp: "up" as const,
  ArrowDown: "down" as const,
  ArrowLeft: "left" as const,
  ArrowRight: "right" as const,
  Up: "up" as const,
  Down: "down" as const,
  Left: "left" as const,
  Right: "right" as const,
  "\uF700": "up" as const,
  "\uF701": "down" as const,
  "\uF702": "left" as const,
  "\uF703": "right" as const,
}[key] || ({ 37: "left" as const, 38: "up" as const, 39: "right" as const, 40: "down" as const })[keyCode] || null);

const lineStart = (value: string, position: number) => value.lastIndexOf("\n", Math.max(0, position - 1)) + 1;
const lineEnd = (value: string, position: number) => {
  const end = value.indexOf("\n", position);
  return end === -1 ? value.length : end;
};

function LoadingDots({ className = "loading-dots" }: { className?: string }) {
  return <span className={className} aria-hidden="true"><i /><i /><i /></span>;
}

function ChatApp() {
  const [sidebarWidth, setSidebarWidth] = useState(280);
  const [workspaceSurface, setWorkspaceSurface] = useState<WorkspaceSurfaceEvent | null>(null);
  const [workspaceStatus, setWorkspaceStatus] = useState<{ message: string; error: boolean } | null>(null);
  const [language, setLanguage] = useState<"en" | "es">("es");
  const [agentName, setAgentName] = useState("Mercury");
  const [agentIdentity, setAgentIdentity] = useState<AgentIdentity>("mercury");
  const [contextLabel, setContextLabel] = useState("AGENTE DE BLACKHOLES");
  const [agentContext, setAgentContext] = useState<HydrateEvent["agent_context"]>(null);
  const [placeholder, setPlaceholder] = useState("Dile a Blackholes en qué proyecto trabajar…");
  const [welcome, setWelcome] = useState("Orquesta proyectos y delega trabajo a tus agentes.");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [composerStatus, setComposerStatus] = useState("");
  const [prompt, setPrompt] = useState("");
  const [pendingImages, setPendingImages] = useState<ChatAttachment[]>([]);
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null);
  const [identityMenuOpen, setIdentityMenuOpen] = useState(false);
  const [permissionMenuOpen, setPermissionMenuOpen] = useState(false);
  const [fullAccess, setFullAccess] = useState(true);
  const [permissionControlSupported, setPermissionControlSupported] = useState(false);
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [providerLabel, setProviderLabel] = useState("Claude");
  const [model, setModel] = useState("automatic");
  const [modelLabel, setModelLabel] = useState("Automático");
  const [modelOptions, setModelOptions] = useState<ChatModelOption[]>([]);
  const [modelCatalogLoading, setModelCatalogLoading] = useState(false);
  const [modelCatalogError, setModelCatalogError] = useState(false);
  const [modelControlSupported, setModelControlSupported] = useState(false);
  const [appModal, setAppModal] = useState<AppModalState | null>(null);
  const [quickOpen, setQuickOpen] = useState<QuickOpenState | null>(null);
  const [showJump, setShowJump] = useState(false);

  const messagesRef = useRef<HTMLElement>(null);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const composerWrapRef = useRef<HTMLElement>(null);
  const receiveRef = useRef<(event: ChatNativeEvent) => void>(() => undefined);
  const workingLabel = language === "en" ? `${agentName} is working…` : `${agentName} está trabajando…`;
  const hasContent = prompt.trim().length > 0 || pendingImages.length > 0;

  const scrollToLatest = (smooth = true) => {
    const element = messagesRef.current;
    if (!element) return;
    element.scrollTo({ top: element.scrollHeight, behavior: smooth ? "smooth" : "auto" });
    setShowJump(false);
  };

  const scrollAfterRender = (smooth = true) => {
    requestAnimationFrame(() => scrollToLatest(smooth));
  };

  const resetComposer = () => {
    setPrompt("");
    setPendingImages([]);
    setEditingMessageId(null);
  };

  const addPendingImage = (attachment?: ChatAttachment) => {
    if (
      !attachment?.id || !validImageTypes.has(attachment.media_type) || !attachment.data ||
      attachment.data.length > 5 * 1024 * 1024 * 4 / 3 + 4
    ) {
      setComposerStatus(language === "en" ? "This image format is not supported." : "Este formato de imagen no es compatible.");
      return;
    }
    setPendingImages((current) => {
      if (current.length >= maxImages) {
        setComposerStatus(language === "en"
          ? `You can attach up to ${maxImages} images.`
          : `Puedes adjuntar hasta ${maxImages} imágenes.`);
        return current;
      }
      setComposerStatus("");
      return [...current, attachment];
    });
    requestAnimationFrame(() => promptRef.current?.focus());
  };

  const insertPastedText = (text = "") => {
    const input = promptRef.current;
    const normalized = String(text).replace(/\r\n?/g, "\n");
    if (!input) {
      setPrompt((current) => current + normalized);
      return;
    }
    const start = input.selectionStart ?? input.value.length;
    const end = input.selectionEnd ?? start;
    const next = input.value.slice(0, start) + normalized + input.value.slice(end);
    setPrompt(next);
    requestAnimationFrame(() => {
      const position = start + normalized.length;
      input.focus();
      input.setSelectionRange(position, position);
    });
  };

  receiveRef.current = (event) => {
    if (event.theme) applyAppTheme(event.theme);
    if (typeof event.sidebar_width === "number" && Number.isFinite(event.sidebar_width)) setSidebarWidth(event.sidebar_width);
    switch (event.type) {
      case "hydrate": {
        const hydrate = event as HydrateEvent;
        setWorkspaceSurface(null);
        const nextLanguage = hydrate.language || "es";
        const nextAgentName = hydrate.agent_name || "Mercury";
        const nextWorkingLabel = nextLanguage === "en"
          ? `${nextAgentName} is working…`
          : `${nextAgentName} está trabajando…`;
        const hydratedMessages = (hydrate.messages || []).map((message) => normalizeMessage(message));
        if (hydrate.active_response) {
          const activeResponse = normalizeMessage({
            id: hydrate.active_response.id,
            role: "assistant",
            content: hydrate.active_response.text || "",
            created_at: hydrate.active_response.created_at || new Date().toISOString(),
            activities: hydrate.active_response.activities || [],
            handoffs: hydrate.active_response.handoffs || [],
            streaming: true,
            activityLabel: hydrate.active_response.text ? nextWorkingLabel : "",
          });
          const afterIndex = hydrate.active_response.after_id
            ? hydratedMessages.findIndex((message) => message.id === hydrate.active_response?.after_id)
            : -1;
          hydratedMessages.splice(afterIndex < 0 ? hydratedMessages.length : afterIndex + 1, 0, activeResponse);
        }
        document.documentElement.lang = nextLanguage;
        setLanguage(nextLanguage);
        setAgentName(nextAgentName);
        setAgentIdentity(normalizeIdentity(hydrate.agent_identity));
        setContextLabel(hydrate.context_label || "AGENTE DE BLACKHOLES");
        setAgentContext(hydrate.agent_context || null);
        setPlaceholder(hydrate.placeholder || "");
        setWelcome(hydrate.welcome || "");
        setMessages(hydratedMessages);
        setBusy(Boolean(hydrate.busy));
        setFullAccess(Boolean(hydrate.full_access));
        setPermissionControlSupported(Boolean(hydrate.permission_control_supported));
        setProviderLabel(hydrate.provider_label || "");
        setModel(hydrate.model || "automatic");
        setModelLabel(hydrate.model_label || hydrate.model || "automatic");
        setModelOptions(hydrate.model_options || []);
        setModelCatalogLoading(Boolean(hydrate.model_catalog_loading));
        setModelCatalogError(Boolean(hydrate.model_catalog_error));
        setModelControlSupported(Boolean(hydrate.model_control_supported));
        setStopping(false);
        setComposerStatus(hydrate.busy ? nextWorkingLabel : "");
        resetComposer();
        setIdentityMenuOpen(false);
        setPermissionMenuOpen(false);
        setModelMenuOpen(false);
        scrollAfterRender(false);
        break;
      }
      case "workspace_surface":
        setWorkspaceSurface(event as WorkspaceSurfaceEvent);
        break;
      case "workspace_status":
        setWorkspaceStatus(event.message ? { message: event.message, error: Boolean(event.error) } : null);
        break;
      case "quick_open":
        setQuickOpen(event as unknown as QuickOpenState);
        setIdentityMenuOpen(false);
        setPermissionMenuOpen(false);
        setModelMenuOpen(false);
        break;
      case "quick_open_close":
        setQuickOpen(null);
        break;
      case "assistant_start": {
        if (!event.id) break;
        setMessages((current) => {
          const existing = current.find((message) => message.id === event.id);
          if (existing) {
            return upsertMessage(current, event.id!, (message) => ({
              ...message,
              created_at: event.created_at || message.created_at,
              streaming: true,
              activityLabel: "",
              error: false,
            }));
          }
          const response = normalizeMessage({
            id: event.id!,
            role: "assistant",
            created_at: event.created_at,
            streaming: true,
          });
          const afterIndex = event.after_id
            ? current.findIndex((message) => message.id === event.after_id)
            : -1;
          if (afterIndex < 0) return [...current, response];
          return [...current.slice(0, afterIndex + 1), response, ...current.slice(afterIndex + 1)];
        });
        setBusy(true);
        setStopping(false);
        setComposerStatus(event.status || workingLabel);
        scrollAfterRender();
        break;
      }
      case "assistant_steered": {
        if (!event.id) break;
        setMessages((current) => {
          const active = current.find((message) => message.id === event.id);
          if (!active) return current;
          return [
            ...current.filter((message) => message.id !== event.id),
            { ...active, streaming: true, activityLabel: workingLabel },
          ];
        });
        setComposerStatus(event.status || workingLabel);
        scrollAfterRender();
        break;
      }
      case "assistant_queued": {
        setComposerStatus(event.status || (language === "en"
          ? "Message queued until the current response finishes"
          : "Mensaje en cola hasta que termine la respuesta actual"));
        break;
      }
      case "assistant_delta": {
        if (!event.id) break;
        const follow = atBottom(messagesRef.current);
        setMessages((current) => upsertMessage(current, event.id!, (message) => ({
          ...message,
          content: message.content + (event.text || ""),
          streaming: true,
          activityLabel: workingLabel,
        })));
        if (follow) scrollAfterRender(false);
        break;
      }
      case "assistant_status": {
        if (!event.id) break;
        const status = event.status || workingLabel;
        setMessages((current) => upsertMessage(current, event.id!, (message) => ({ ...message, activityLabel: status })));
        setComposerStatus(status);
        break;
      }
      case "assistant_tool": {
        if (!event.id || !event.activity) break;
        const follow = atBottom(messagesRef.current);
        setMessages((current) => upsertMessage(current, event.id!, (message) => ({
          ...message,
          activities: [...message.activities, event.activity!],
        })));
        if (follow) scrollAfterRender(false);
        break;
      }
      case "assistant_background_task": {
        if (!event.id || !event.activity?.task_id) break;
        const follow = atBottom(messagesRef.current);
        setMessages((current) => upsertMessage(current, event.id!, (message) => {
          const index = message.activities.findIndex((activity) => activity.task_id === event.activity?.task_id);
          if (index < 0) return { ...message, activities: [...message.activities, event.activity!] };
          return {
            ...message,
            activities: message.activities.map((activity, activityIndex) => (
              activityIndex === index ? { ...activity, ...event.activity } : activity
            )),
          };
        }));
        if (follow) scrollAfterRender(false);
        break;
      }
      case "assistant_handoff": {
        if (!event.id || !event.handoff) break;
        const follow = atBottom(messagesRef.current);
        setMessages((current) => upsertMessage(current, event.id!, (message) => {
          const key = handoffKey(event.handoff!);
          if (message.handoffs.some((handoff) => handoffKey(handoff) === key)) return message;
          return { ...message, handoffs: [...message.handoffs, event.handoff!] };
        }));
        if (follow) scrollAfterRender(false);
        break;
      }
      case "assistant_done": {
        if (event.id) {
          setMessages((current) => upsertMessage(current, event.id!, (message) => ({
            ...message,
            duration_ms: event.duration_ms,
            streaming: false,
            activityLabel: "",
          })));
        }
        setBusy(false);
        setStopping(false);
        setComposerStatus("");
        scrollAfterRender();
        requestAnimationFrame(() => promptRef.current?.focus());
        break;
      }
      case "assistant_stopped": {
        if (event.id) {
          setMessages((current) => upsertMessage(current, event.id!, (message) => ({
            ...message,
            duration_ms: event.duration_ms,
            content: message.content || event.fallback || (language === "en" ? "Response stopped." : "Respuesta detenida."),
            streaming: false,
            activityLabel: "",
            interrupted: true,
            interruptedLabel: event.label,
          })));
        }
        setBusy(false);
        setStopping(false);
        setComposerStatus(event.status || "");
        scrollAfterRender();
        requestAnimationFrame(() => promptRef.current?.focus());
        break;
      }
      case "error": {
        const errorId = event.id || createId();
        setMessages((current) => {
          let next = current;
          if (event.response_id && event.response_id !== errorId) {
            next = upsertMessage(next, event.response_id, (message) => ({
              ...message,
              duration_ms: event.duration_ms,
              streaming: false,
              activityLabel: "",
            }));
          }
          return upsertMessage(next, errorId, (message) => ({
            ...message,
            duration_ms: event.response_id === errorId ? event.duration_ms : null,
            content: event.message || "",
            streaming: false,
            activityLabel: "",
            error: true,
          }));
        });
        setBusy(false);
        setStopping(false);
        setComposerStatus("");
        scrollAfterRender();
        requestAnimationFrame(() => promptRef.current?.focus());
        break;
      }
      case "new_chat":
        setMessages([]);
        setBusy(false);
        setStopping(false);
        setComposerStatus("");
        resetComposer();
        requestAnimationFrame(() => promptRef.current?.focus());
        break;
      case "paste":
        insertPastedText(event.text || "");
        break;
      case "paste_image":
        addPendingImage(event.attachment);
        break;
      case "composer_notice":
        setComposerStatus(event.message || "");
        break;
      case "permissions_changed":
        setFullAccess(Boolean(event.full_access));
        setPermissionControlSupported(Boolean(event.permission_control_supported));
        break;
      case "model_changed":
        setProviderLabel(event.provider_label || "");
        setModel(event.model || "automatic");
        setModelLabel(event.model_label || event.model || "automatic");
        setModelControlSupported(Boolean(event.model_control_supported));
        setModelOptions(event.model_options || []);
        setModelCatalogLoading(Boolean(event.model_catalog_loading));
        setModelCatalogError(Boolean(event.model_catalog_error));
        break;
      case "app_modal":
        setAppModal(event.modal || null);
        setIdentityMenuOpen(false);
        setPermissionMenuOpen(false);
        setModelMenuOpen(false);
        break;
      case "app_modal_feedback":
        setAppModal(current => current && current.request_id === event.request_id
          ? { ...current, feedback: event.feedback } : current);
        break;
    }
  };

  useEffect(() => {
    window.blackholesNative = { receive: (event) => receiveRef.current(event as ChatNativeEvent) };
    postNative({ type: "ready" });
    return () => { delete window.blackholesNative; };
  }, []);

  useEffect(() => {
    const closeMenu = () => {
      setIdentityMenuOpen(false);
      setPermissionMenuOpen(false);
      setModelMenuOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeMenu();
    };
    document.addEventListener("click", closeMenu);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("click", closeMenu);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  useLayoutEffect(() => {
    const input = promptRef.current;
    if (!input) return;
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, 156)}px`;
  }, [pendingImages, prompt]);

  useLayoutEffect(() => {
    if (workspaceSurface) return;
    const wrap = composerWrapRef.current;
    if (!wrap) return;
    const updateComposerClearance = () => {
      const followLatest = atBottom(messagesRef.current);
      document.documentElement.style.setProperty("--composer-clearance", `${wrap.getBoundingClientRect().height + 28}px`);
      if (followLatest) requestAnimationFrame(() => scrollToLatest(false));
    };
    updateComposerClearance();
    const observer = new ResizeObserver(updateComposerClearance);
    observer.observe(wrap);
    return () => observer.disconnect();
  }, [workspaceSurface]);

  useEffect(() => {
    const shortcutLetter = (event: KeyboardEvent) => {
      if (typeof event.code === "string" && /^Key[A-Z]$/.test(event.code)) return event.code.slice(3).toLowerCase();
      if (event.keyCode >= 65 && event.keyCode <= 90) return String.fromCharCode(event.keyCode).toLowerCase();
      return typeof event.key === "string" && event.key.length === 1 ? event.key.toLowerCase() : "";
    };
    const handler = (event: KeyboardEvent) => {
      if (!event.metaKey && !event.ctrlKey) return;
      const input = promptRef.current;
      const letter = shortcutLetter(event);
      if (document.activeElement !== input) {
        if (letter !== "c") return;
        const selectedText = window.getSelection()?.toString() || "";
        if (!selectedText) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        postNative({ type: "copy_text", text: selectedText });
        return;
      }
      if (!input || !["a", "c", "v", "x"].includes(letter)) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (letter === "a") {
        input.setSelectionRange(0, input.value.length, "forward");
        return;
      }
      if (letter === "v") {
        postNative({ type: "request_paste" });
        return;
      }
      const start = input.selectionStart ?? 0;
      const end = input.selectionEnd ?? start;
      if (start === end) return;
      postNative({ type: "copy_text", text: input.value.slice(start, end) });
      if (letter === "x") {
        const next = input.value.slice(0, start) + input.value.slice(end);
        setPrompt(next);
        requestAnimationFrame(() => input.setSelectionRange(start, start));
      }
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, []);

  const movePromptCaret = (direction: "up" | "down" | "left" | "right", event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const input = promptRef.current;
    if (!input) return;
    const value = input.value;
    const start = input.selectionStart ?? value.length;
    const end = input.selectionEnd ?? start;
    const selectionDirection = input.selectionDirection || "none";
    const anchor = selectionDirection === "backward" ? end : start;
    const focus = selectionDirection === "backward" ? start : end;
    let next = focus;

    if (event.metaKey) {
      if (direction === "left") next = lineStart(value, focus);
      if (direction === "right") next = lineEnd(value, focus);
      if (direction === "up") next = 0;
      if (direction === "down") next = value.length;
    } else if (direction === "left") {
      next = Math.max(0, focus - 1);
    } else if (direction === "right") {
      next = Math.min(value.length, focus + 1);
    } else {
      const currentStart = lineStart(value, focus);
      const column = focus - currentStart;
      if (direction === "up" && currentStart > 0) {
        const previousEnd = currentStart - 1;
        next = Math.min(lineStart(value, previousEnd) + column, previousEnd);
      } else if (direction === "down") {
        const currentEnd = lineEnd(value, focus);
        if (currentEnd < value.length) {
          const followingStart = currentEnd + 1;
          next = Math.min(followingStart + column, lineEnd(value, followingStart));
        }
      }
    }

    if (!event.shiftKey) {
      if (start !== end) {
        if (direction === "left" || direction === "up") next = start;
        if (direction === "right" || direction === "down") next = end;
      }
      input.setSelectionRange(next, next, "none");
      return;
    }
    input.setSelectionRange(Math.min(anchor, next), Math.max(anchor, next), next < anchor ? "backward" : "forward");
  };

  const sendPrompt = () => {
    const content = prompt.trim();
    if ((!content && pendingImages.length === 0) || stopping) return;
    const id = createId();
    const createdAt = new Date().toISOString();
    const attachments = pendingImages.map((attachment) => ({ ...attachment }));
    const editedMessageId = editingMessageId;

    if (!editedMessageId) {
      const userMessage = normalizeMessage({ id, role: "user", content, created_at: createdAt, attachments });
      setMessages((current) => [...current, userMessage]);
    }
    resetComposer();
    if (busy) {
      setComposerStatus(providerLabel === "Claude"
        ? (language === "en" ? "Interrupting to answer…" : "Interrumpiendo para responder…")
        : (language === "en"
            ? "Message queued until the current response finishes"
            : "Mensaje en cola hasta que termine la respuesta actual"));
    } else {
      setBusy(true);
      setComposerStatus(workingLabel);
    }
    scrollAfterRender();
    postNative({
      type: editedMessageId ? "edit_message" : "send_message",
      message_id: editedMessageId || undefined,
      id,
      message: content,
      created_at: createdAt,
      attachments,
    });
  };

  const beginEditing = (message: ChatMessage) => {
    if (busy || message.role !== "user") return;
    setEditingMessageId(message.id);
    setPrompt(message.content || "");
    setPendingImages(message.attachments.map((attachment) => ({ ...attachment })));
    requestAnimationFrame(() => {
      const input = promptRef.current;
      if (!input) return;
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    });
  };

  const onPromptKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const direction = arrowKey(event.key, event.keyCode || event.which || 0);
    if (direction) {
      event.preventDefault();
      event.stopPropagation();
      movePromptCaret(direction, event);
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      sendPrompt();
    }
  };

  const onPaste = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const imageItems = Array.from(event.clipboardData?.items || [])
      .filter((item) => item.kind === "file" && item.type.startsWith("image/"));
    if (imageItems.length === 0) return;
    event.preventDefault();
    for (const item of imageItems) {
      const file = item.getAsFile();
      if (!file) continue;
      if (file.size > 5 * 1024 * 1024) {
        setComposerStatus(language === "en" ? "The image is larger than the 5 MB limit." : "La imagen supera el límite de 5 MB.");
        continue;
      }
      const reader = new FileReader();
      reader.addEventListener("load", () => {
        const match = String(reader.result || "").match(/^data:([^;]+);base64,(.+)$/s);
        if (match) addPendingImage({ id: createId(), media_type: match[1], data: match[2] });
      });
      reader.readAsDataURL(file);
    }
  };

  const renderedMessages = useMemo(() => {
    const rows: React.ReactNode[] = [];
    let previousDay = "";
    for (const message of messages) {
      const day = new Date(message.created_at).toDateString();
      if (day !== previousDay) {
        previousDay = day;
        rows.push(<div className="day-separator" key={`day-${message.id}`}>{formatDay(message.created_at, language)}</div>);
      }
      rows.push(
        <MessageView
          key={message.id}
          message={message}
          language={language}
          agentName={agentName}
          busy={busy}
          onEdit={beginEditing}
        />,
      );
    }
    return rows;
  }, [agentName, busy, language, messages]);

  const selectIdentity = (identity: AgentIdentity) => {
    setAgentIdentity(identity);
    setAgentName(agentIdentityName(identity));
    setIdentityMenuOpen(false);
    postNative({ type: "set_agent_identity", identity });
  };

  const selectPermissionMode = (enabled: boolean) => {
    setFullAccess(enabled);
    setPermissionMenuOpen(false);
    postNative({ type: "set_agents_full_access", enabled });
  };

  const selectModel = (option: ChatModelOption) => {
    setModel(option.value);
    setModelLabel(option.label);
    setModelMenuOpen(false);
    postNative({ type: "set_agent_model", model: option.value });
  };

  const actionLabel = language === "en" ? "Send message" : "Enviar mensaje";
  const stopLabel = stopping
    ? (language === "en" ? "Stopping agent…" : "Deteniendo agente…")
    : (language === "en" ? "Stop agent" : "Detener agente");

  const dismissAppModal = () => {
    setAppModal(null);
    postNative({ type: "dismiss_app_modal" });
  };

  const appModalLayer = appModal && <AppModal key={appModal.request_id || `${appModal.kind}:${appModal.scope || appModal.workspace_id || appModal.task_id}`}
    modal={appModal} language={language} onDismiss={dismissAppModal} />;

  const quickOpenLayer = quickOpen && <QuickOpen state={quickOpen} />;
  const sidebarDivider = !appModal && !quickOpen && <SidebarResizeHandle width={sidebarWidth}
    left={0} hitWidth={8} edge="left" fixed label={language === "en" ? "Resize sidebar" : "Cambiar ancho del menú lateral"} />;

  if (workspaceSurface) {
    return (
      <>
        <WorkspaceSurface event={workspaceSurface} />
        {workspaceSurface.surface !== "settings" && sidebarDivider}
        {workspaceStatus && (
          <button
            type="button"
            className={`workspace-status${workspaceStatus.error ? " is-error" : ""}`}
            onClick={() => {
              setWorkspaceStatus(null);
              postNative({ type: "dismiss_status" });
            }}
          >{workspaceStatus.message}</button>
        )}
        {appModalLayer}
        {quickOpenLayer}
      </>
    );
  }

  return (
    <main className="chat-shell" aria-label="Blackholes Orchestrator">
      {sidebarDivider}
      <header className="chat-agent-header">
        <button
          className="chat-agent-trigger"
          type="button"
          aria-haspopup="menu"
          aria-expanded={identityMenuOpen}
          title={language === "en" ? "Change bot" : "Cambiar bot"}
          onClick={(event) => {
            event.stopPropagation();
            setPermissionMenuOpen(false);
            setModelMenuOpen(false);
            setIdentityMenuOpen((open) => !open);
          }}
        >
          <span className="chat-avatar-wrap" aria-hidden="true">
            <span className="black-bot-avatar"><AgentAvatar identity={agentIdentity} size={26} busy={busy} /></span>
            <span className="agent-active-badge" hidden={!busy} />
          </span>
          <span>{agentName}</span>
          {busy && <LoadingDots className="header-working-dots" />}
        </button>
        {agentContext && <nav className="chat-context-path" aria-label={language === "en" ? "Agent location" : "Ubicación del agente"}>
          <button type="button" title={agentContext.project_label}
            aria-label={`${language === "en" ? "Locate project" : "Localizar proyecto"}: ${agentContext.project_label}`}
            onClick={() => postNative({ type: "reveal_agent_context", project_only: true })}>
            {agentContext.project_label}
          </button>
          {agentContext.kind === "task" && <>
            <span aria-hidden="true">/</span>
            <button type="button" title={agentContext.label}
              aria-label={`${language === "en" ? "Locate task" : "Localizar tarea"}: ${agentContext.label}`}
              onClick={() => postNative({ type: "reveal_agent_context", project_only: false })}>
              {agentContext.label}
            </button>
          </>}
        </nav>}
        <div className="agent-identity-menu" role="menu" hidden={!identityMenuOpen} onClick={(event) => event.stopPropagation()}>
          <span>{language === "en" ? "Choose a bot" : "Elige un bot"}</span>
          <div className="agent-identity-grid">
            {(["mercury", "earthy", "saturny"] as AgentIdentity[]).map((identity) => (
              <button
                key={identity}
                type="button"
                className={agentIdentity === identity ? "is-selected" : ""}
                aria-label={agentIdentityName(identity)}
                aria-checked={agentIdentity === identity}
                role="menuitemradio"
                onClick={() => selectIdentity(identity)}
              >
                <span className="agent-identity-preview" aria-hidden="true"><AgentAvatar identity={identity} size={72} /></span>
                <strong>{agentIdentityName(identity)}</strong>
              </button>
            ))}
          </div>
        </div>
        <button
          className="chat-header-action"
          type="button"
          aria-label={language === "en" ? "New conversation" : "Nueva conversación"}
          title={language === "en" ? "New conversation" : "Nueva conversación"}
          onClick={() => postNative({ type: "new_chat" })}
        >
          <SquarePen size={17} />
        </button>
      </header>

      <section className={`welcome${messages.length > 0 ? " is-hidden" : ""}`} aria-live="polite">
        <span className="welcome__avatar" aria-hidden="true">
          <AgentAvatar identity={agentIdentity} size={92} busy={busy} />
        </span>
        <span className="welcome__context">{contextLabel}</span>
        <h1>{agentName}</h1>
        <p>{welcome}</p>
      </section>

      <section
        ref={messagesRef}
        className="messages"
        aria-live="polite"
        onScroll={(event) => setShowJump(!atBottom(event.currentTarget) && messages.length > 0)}
      >
        {renderedMessages}
      </section>

      <button
        className={`jump-latest${showJump ? " is-visible" : ""}`}
        type="button"
        aria-label={language === "en" ? "Go to latest message" : "Ir al último mensaje"}
        onClick={() => scrollToLatest()}
      >
        <ChevronDown size={24} />
      </button>

      <footer ref={composerWrapRef} className="composer-wrap">
        <form
          className={[
            "composer",
            editingMessageId ? "is-editing" : "",
            busy ? "is-busy" : "",
            stopping ? "is-stopping" : "",
            pendingImages.length ? "has-attachments" : "",
          ].filter(Boolean).join(" ")}
          onSubmit={(event) => {
            event.preventDefault();
            if (hasContent) sendPrompt();
          }}
        >
          <div className="composer-editing" hidden={!editingMessageId}>
            <span>{language === "en" ? "Editing message" : "Editando mensaje"}</span>
            <button
              type="button"
              aria-label={language === "en" ? "Cancel editing" : "Cancelar edición"}
              onClick={() => {
                resetComposer();
                requestAnimationFrame(() => promptRef.current?.focus());
              }}
            >
              <X size={13} />
            </button>
          </div>
          <div className="composer-attachments" hidden={pendingImages.length === 0}>
            {pendingImages.map((attachment) => (
              <div className="composer-attachment" key={attachment.id}>
                <img src={attachmentDataUrl(attachment)} alt={language === "en" ? "Image ready to send" : "Imagen lista para enviar"} />
                <button
                  type="button"
                  className="composer-attachment__remove"
                  aria-label={language === "en" ? "Remove image" : "Quitar imagen"}
                  onClick={() => {
                    setPendingImages((current) => current.filter((image) => image.id !== attachment.id));
                    requestAnimationFrame(() => promptRef.current?.focus());
                  }}
                >
                  <X size={12} />
                </button>
              </div>
            ))}
          </div>
          <div className="composer-tools">
            <button
              className="composer__icon"
              type="button"
              aria-label={language === "en" ? "Attach images" : "Adjuntar imágenes"}
              title={language === "en" ? "Attach images" : "Adjuntar imágenes"}
              onClick={() => postNative({ type: "choose_attachments" })}
            >
              <Plus size={20} />
            </button>
            {permissionControlSupported && (
              <div className="composer-permission" onClick={(event) => event.stopPropagation()}>
                <button
                  className={`composer-permission__trigger${fullAccess ? " is-full" : ""}`}
                  type="button"
                  aria-haspopup="menu"
                  aria-expanded={permissionMenuOpen}
                  title={language === "en" ? "Agent permissions" : "Permisos del agente"}
                  onClick={() => {
                    setIdentityMenuOpen(false);
                    setModelMenuOpen(false);
                    setPermissionMenuOpen((open) => !open);
                  }}
                >
                  <ShieldCheck size={14} />
                  <span>{fullAccess ? (language === "en" ? "Full access" : "Acceso total") : (language === "en" ? "Standard" : "Estándar")}</span>
                  <ChevronDown size={13} />
                </button>
                <div className="composer-permission__menu" role="menu" hidden={!permissionMenuOpen}>
                  <button type="button" role="menuitemradio" aria-checked={fullAccess} onClick={() => selectPermissionMode(true)}>
                    <span><strong>{language === "en" ? "Full access" : "Acceso total"}</strong><small>{language === "en" ? "No permission prompts" : "Sin confirmaciones de permisos"}</small></span>
                    {fullAccess && <Check size={15} />}
                  </button>
                  <button type="button" role="menuitemradio" aria-checked={!fullAccess} onClick={() => selectPermissionMode(false)}>
                    <span><strong>{language === "en" ? "Standard" : "Estándar"}</strong><small>{language === "en" ? "Provider safety checks" : "Controles de seguridad del proveedor"}</small></span>
                    {!fullAccess && <Check size={15} />}
                  </button>
                </div>
              </div>
            )}
            {providerLabel && modelLabel && (
              <div className="composer-model" onClick={(event) => event.stopPropagation()}>
                <button
                  className="composer-model__trigger"
                  type="button"
                  aria-haspopup={modelControlSupported ? "menu" : undefined}
                  aria-expanded={modelControlSupported ? modelMenuOpen : undefined}
                  disabled={!modelControlSupported}
                  title={`${providerLabel} · ${modelLabel}`}
                  onClick={() => {
                    setIdentityMenuOpen(false);
                    setPermissionMenuOpen(false);
                    setModelMenuOpen((open) => !open);
                  }}
                >
                  <Zap size={13} />
                  <span><strong>{providerLabel}</strong> · {modelLabel}</span>
                  {modelControlSupported && <ChevronDown size={13} />}
                </button>
                {modelControlSupported && (
                  <div className="composer-model__menu" role="menu" hidden={!modelMenuOpen}>
                    <span>{language === "en" ? `${providerLabel} model` : `Modelo de ${providerLabel}`}</span>
                    {(modelCatalogLoading || modelCatalogError) && <span role="status">{modelCatalogLoading
                      ? (language === "en" ? "Loading account models…" : "Cargando modelos de la cuenta…")
                      : (language === "en" ? "Catalog unavailable. Check account settings." : "Catálogo no disponible. Revisa la cuenta en ajustes.")}</span>}
                    {modelOptions.map((option) => (
                      <button
                        key={option.value}
                        type="button"
                        role="menuitemradio"
                        disabled={option.disabled}
                        aria-checked={option.value === model}
                        onClick={() => selectModel(option)}
                      >
                        <span>{option.label}</span>
                        {option.value === model && <Check size={15} />}
                      </button>
                    ))}
                    <button className="composer-model__refresh" type="button" role="menuitem" disabled={modelCatalogLoading} onClick={() => postNative({ type: "refresh_model_catalog", force: true })}>
                      {language === "en" ? "Refresh models" : "Actualizar modelos"}
                    </button>
                  </div>
                )}
              </div>
            )}
          </div>
          <textarea
            ref={promptRef}
            rows={1}
            autoComplete="off"
            spellCheck
            placeholder={placeholder}
            aria-label={language === "en" ? "Message for Blackholes" : "Mensaje para Blackholes"}
            value={prompt}
            onChange={(event) => setPrompt(event.target.value.replace(invalidNavigationCharacters, ""))}
            onBeforeInput={(event) => {
              const data = (event.nativeEvent as InputEvent).data;
              if (typeof data === "string" && invalidNavigationCharacter.test(data)) event.preventDefault();
            }}
            onKeyDown={onPromptKeyDown}
            onPaste={onPaste}
          />
          <button
            className="composer__stop"
            type="button"
            aria-label={stopLabel}
            title={stopLabel}
            hidden={!busy}
            disabled={stopping}
            onClick={() => {
              if (!busy || stopping) return;
              setStopping(true);
              setComposerStatus(language === "en" ? "Stopping agent…" : "Deteniendo agente…");
              postNative({ type: "stop_agent" });
            }}
          >
            <Square size={15} />
          </button>
          <button className="composer__action" type="submit" aria-label={actionLabel} title={actionLabel} disabled={!hasContent || stopping}>
            <ArrowUp size={17} />
          </button>
        </form>
        <div className="composer-status" role="status">
          {composerStatus && <><span>{composerStatus}</span>{busy && <LoadingDots />}</>}
        </div>
      </footer>
      {workspaceStatus && (
        <button
          type="button"
          className={`workspace-status${workspaceStatus.error ? " is-error" : ""}`}
          onClick={() => {
            setWorkspaceStatus(null);
            postNative({ type: "dismiss_status" });
          }}
        >{workspaceStatus.message}</button>
      )}
      {appModalLayer}
      {quickOpenLayer}
    </main>
  );
}

const root = document.querySelector<HTMLElement>("#root");
if (!root) throw new Error("Blackholes chat root was not found.");
createRoot(root).render(
  <StrictMode>
    <ChatApp />
  </StrictMode>,
);

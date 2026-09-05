import { Check, ChevronLeft, ChevronRight, Copy, FolderOpen, ListTodo, Pencil } from "lucide-react";
import { useMemo, useState } from "react";
import { markdownToHtml, safeLink } from "./markdown";
import type { ChatHandoff, ChatMessage } from "./types";
import { postNative } from "../shared/native";
import { AgentAvatar } from "../shared/AgentAvatar";
import { ActivityTimeline } from "./ActivityTimeline";

const attachmentDataUrl = (attachment: ChatMessage["attachments"][number]) => (
  `data:${attachment.media_type};base64,${attachment.data}`
);

const handoffKey = (handoff: ChatHandoff): string => (
  `${handoff.navigation ? "navigation" : "handoff"}:${handoff.scope}:${handoff.task_id || handoff.project_id || ""}`
);

export function MessageView({ message, language, agentName, busy, onEdit }: {
  message: ChatMessage;
  language: "en" | "es";
  agentName: string;
  busy: boolean;
  onEdit(message: ChatMessage): void;
}) {
  const markdown = useMemo(
    () => markdownToHtml(message.content || "", language),
    [language, message.content],
  );
  const handoffs = useMemo(() => {
    const seen = new Set<string>();
    return message.handoffs.filter((handoff) => {
      const key = handoffKey(handoff);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }, [message.handoffs]);
  const [copiedText, setCopiedText] = useState<string | null>(null);
  const hasTimeline = handoffs.length > 0 || message.interrupted;
  const copyMessage = () => {
    postNative({ type: "copy_text", text: message.content });
    setCopiedText(message.content);
  };

  const onContentClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const copyButton = target.closest<HTMLButtonElement>(".markdown-code__copy");
    if (copyButton) {
      const code = copyButton.closest(".markdown-code")?.querySelector("code")?.textContent || "";
      if (!code) return;
      postNative({ type: "copy_text", text: code });
      const previous = copyButton.textContent;
      copyButton.textContent = language === "en" ? "Copied" : "Copiado";
      window.setTimeout(() => { copyButton.textContent = previous; }, 1_100);
      return;
    }
    const link = target.closest<HTMLAnchorElement>("a.markdown-link");
    if (!link) return;
    event.preventDefault();
    const url = safeLink(link.getAttribute("href") || "");
    if (url) postNative({ type: "open_url", url });
  };

  return (
    <article className={`message-row message-row--${message.role}`} data-message-id={message.id}>
      <div className="message-stack">
        <div className={`message${message.error ? " is-error" : ""}${message.attachments.length ? " has-attachments" : ""}`}>
          {message.role === "assistant" && <ActivityTimeline message={message} language={language} agentName={agentName} />}
          <div className="message__attachments" hidden={message.attachments.length === 0}>
            {message.attachments.map((attachment) => (
              <img
                key={attachment.id}
                src={attachmentDataUrl(attachment)}
                alt={language === "en" ? "Attached image" : "Imagen adjunta"}
                loading="lazy"
              />
            ))}
          </div>
          <div
            className="message__content markdown-body"
            hidden={!message.content}
            onClick={onContentClick}
            dangerouslySetInnerHTML={{ __html: markdown }}
          />
          {hasTimeline && (
            <div className="message__timeline">
              {message.interrupted && (
                <div className="agent-interrupted">
                  {message.interruptedLabel || (language === "en" ? "Stopped" : "Detenido")}
                </div>
              )}
              {handoffs.map((handoff) => {
                const fallback = handoff.navigation
                  ? (language === "en" ? "Open" : "Abrir")
                  : (language === "en" ? "Open agent" : "Abrir agente");
                const eyebrow = handoff.navigation
                  ? handoff.scope === "task"
                    ? (language === "en" ? "OPEN TASK" : "ABRIR TAREA")
                    : (language === "en" ? "OPEN PROJECT" : "ABRIR PROYECTO")
                  : (language === "en" ? "DELEGATED WORK" : "TRABAJO DELEGADO");
                return (
                  <button
                    key={handoffKey(handoff)}
                    className={`agent-handoff${handoff.navigation ? " is-navigation" : ""}`}
                    type="button"
                    aria-label={`${language === "en" ? "Open" : "Abrir"} ${handoff.label || fallback}`}
                    title={handoff.label || fallback}
                    onClick={() => postNative({
                      type: handoff.navigation ? "open_target" : "open_agent",
                      scope: handoff.scope,
                      project_id: handoff.project_id || null,
                      task_id: handoff.task_id || null,
                    })}
                  >
                    <span className="agent-handoff__avatar">
                      {handoff.navigation
                        ? handoff.scope === "task" ? <ListTodo size={23} /> : <FolderOpen size={23} />
                        : <AgentAvatar identity={handoff.identity} size={38} />}
                    </span>
                    <span className="agent-handoff__copy">
                      <span>{eyebrow}</span>
                      <strong>{handoff.label || fallback}</strong>
                    </span>
                    <span className="agent-handoff__arrow">›</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
        <div className={`message__actions${message.branch_navigation && message.branch_navigation.total > 1 ? " has-branches" : ""}`}>
          {message.content && !message.streaming && (
            <button type="button" className="message-action"
              aria-label={copiedText === message.content ? (language === "en" ? "Copied" : "Copiado") : (language === "en" ? "Copy message" : "Copiar mensaje")}
              title={language === "en" ? "Copy message" : "Copiar mensaje"}
              onClick={copyMessage}>
              {copiedText === message.content ? <Check size={15} /> : <Copy size={15} />}
            </button>
          )}
          {message.role === "user" && (
            <>
              <button
                type="button"
                className="message-action message-action--edit"
                aria-label={language === "en" ? "Edit message" : "Editar mensaje"}
                disabled={busy}
                onClick={() => onEdit(message)}
              >
                <Pencil size={14} />
              </button>
              {message.branch_navigation && message.branch_navigation.total > 1 && (
                <span className="branch-navigation">
                  <button
                    type="button"
                    className="message-action"
                    disabled={!message.branch_navigation.previous_branch_id}
                    aria-label={language === "en" ? "Previous version" : "Versión anterior"}
                    onClick={() => message.branch_navigation?.previous_branch_id && postNative({
                      type: "switch_branch",
                      branch_id: message.branch_navigation.previous_branch_id,
                    })}
                  >
                    <ChevronLeft size={14} />
                  </button>
                  <span>{message.branch_navigation.position} / {message.branch_navigation.total}</span>
                  <button
                    type="button"
                    className="message-action"
                    disabled={!message.branch_navigation.next_branch_id}
                    aria-label={language === "en" ? "Next version" : "Versión siguiente"}
                    onClick={() => message.branch_navigation?.next_branch_id && postNative({
                      type: "switch_branch",
                      branch_id: message.branch_navigation.next_branch_id,
                    })}
                  >
                    <ChevronRight size={14} />
                  </button>
                </span>
              )}
            </>
          )}
        </div>
      </div>
    </article>
  );
}

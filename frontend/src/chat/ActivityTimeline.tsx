import { BookOpen, ChevronRight, FilePenLine, Search, Terminal, Wrench } from "lucide-react";
import { useEffect, useState } from "react";
import type { ChatMessage } from "./types";

const durationLabel = (milliseconds: number) => {
  const seconds = Math.max(1, Math.floor(milliseconds / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
};

const toolIcon = (tool: string) => {
  if (/bash|shell|command|terminal|exec/i.test(tool)) return Terminal;
  if (/write|edit|patch/i.test(tool)) return FilePenLine;
  if (/search|grep|glob|find/i.test(tool)) return Search;
  if (/read|file|fetch/i.test(tool)) return BookOpen;
  return Wrench;
};

export function ActivityTimeline({ message, language, agentName }: {
  message: ChatMessage;
  language: "en" | "es";
  agentName: string;
}) {
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!message.streaming) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [message.streaming, message.created_at]);

  const en = language === "en";
  const start = Date.parse(message.created_at);
  const elapsed = message.streaming && Number.isFinite(start)
    ? Math.max(0, now - start) : message.duration_ms;
  const timed = typeof elapsed === "number" && Number.isFinite(elapsed);
  const state = message.interrupted ? (en ? "Stopped" : "Detenido")
    : message.error ? (en ? "Failed" : "Falló")
    : message.streaming ? (en ? "Working" : "Trabajando") : (en ? "Worked for" : "Trabajó durante");
  const label = timed ? `${state} ${durationLabel(elapsed)}`
    : message.streaming ? state : (en ? "Agent activity" : "Actividad del agente");
  const statuses: Record<string, string> = en
    ? { running: "Running", foreground: "Running", completed: "Completed", failed: "Failed", stopped: "Stopped", blocked: "Blocked", unknown: "No final status reported" }
    : { running: "Ejecutando", foreground: "Ejecutando", completed: "Completado", failed: "Falló", stopped: "Detenido", blocked: "Bloqueado", unknown: "Sin estado final reportado" };
  const hasActivities = message.activities.length > 0;
  if (!hasActivities && !message.streaming && !timed) return null;

  const summary = <>
    {message.streaming && <span className="work-log__pulse" aria-hidden="true" />}
    <span>{label}</span>
    {hasActivities && <><span className="work-log__count">{message.activities.length}</span><ChevronRight className="work-log__chevron" size={15} aria-hidden="true" /></>}
  </>;

  return <div className={`work-log${message.streaming ? " is-running" : ""}`}>
    {hasActivities ? <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary title={en ? "Show tools and commands" : "Ver herramientas y comandos"}>{summary}</summary>
      {open && <div className="work-log__entries">
        {message.activities.map((activity, index) => {
          const Icon = toolIcon(activity.tool || "");
          // A stopped/finished turn must not look as though its old process is still running.
          const status = !message.streaming && ["running", "foreground"].includes(activity.status || "")
            ? (message.interrupted ? "stopped" : "unknown") : activity.status;
          return <div className={`work-log__entry is-${status || "activity"}`} key={`${activity.task_id || activity.created_at || index}-${index}`}>
            <Icon size={16} aria-hidden="true" />
            <div className="work-log__entry-body">
              <div className="work-log__heading">
                <span title={activity.agent || agentName}>{activity.tool || (en ? "Tool" : "Herramienta")}</span>
                {status && <small>{statuses[status] || status}</small>}
              </div>
              {activity.detail && <pre>{activity.detail}</pre>}
              {activity.summary && activity.summary !== activity.detail && <p>{activity.summary}</p>}
            </div>
          </div>;
        })}
      </div>}
    </details> : <div className="work-log__label">{summary}</div>}
    {message.streaming && message.activityLabel && <div className="work-log__current" role="status">{message.activityLabel}</div>}
  </div>;
}

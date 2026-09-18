import { receiveSourceControlResult } from "./SourceChanges";
import { receiveTaskDetailsSave } from "./TaskDetails";
import { StrictMode, useEffect, useLayoutEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { postNative } from "../shared/native";
import { applyAppTheme, type AppTheme } from "../shared/theme";
import { SidebarResizeHandle } from "../shared/SidebarResizeHandle";
import { AppModal } from "./AppModal";
import { QuickOpen, type QuickOpenState } from "./QuickOpen";
import type { AppModalState } from "./types";
import { WorkspaceSurface, type WorkspaceSurfaceEvent } from "./WorkspaceSurface";

type WorkspaceEvent = {
  type: string;
  theme?: AppTheme;
  language?: "en" | "es";
  sidebar_width?: number;
  message?: string;
  error?: boolean;
  modal?: AppModalState | null;
  request_id?: string;
  feedback?: AppModalState["feedback"];
};

function WorkspaceApp() {
  const [sidebarWidth, setSidebarWidth] = useState(280);
  const [surface, setSurface] = useState<WorkspaceSurfaceEvent | null>(null);
  const [status, setStatus] = useState<{ message: string; error: boolean } | null>(null);
  const [language, setLanguage] = useState<"en" | "es">("es");
  const [modal, setModal] = useState<AppModalState | null>(null);
  const [quickOpen, setQuickOpen] = useState<QuickOpenState | null>(null);
  const overTerminal = Boolean(modal?.over_terminal || quickOpen?.over_terminal);

  useLayoutEffect(() => {
    document.documentElement.classList.toggle("native-terminal-overlay", overTerminal);
    return () => document.documentElement.classList.remove("native-terminal-overlay");
  }, [overTerminal]);

  useEffect(() => {
    window.blackholesNative = { receive(raw) {
      if (!raw || typeof raw !== "object") return;
      const event = raw as WorkspaceEvent;
      if (event.theme) applyAppTheme(event.theme);
      if (typeof event.sidebar_width === "number" && Number.isFinite(event.sidebar_width)) setSidebarWidth(event.sidebar_width);
      if (event.language) {
        setLanguage(event.language);
        document.documentElement.lang = event.language;
      }
      switch (event.type) {
        case "workspace_surface": {
          const next = raw as WorkspaceSurfaceEvent;
          setSurface(next);
          if ("language" in next.data) {
            setLanguage(next.data.language);
            document.documentElement.lang = next.data.language;
          }
          break;
        }
        case "source_control_result":
          receiveSourceControlResult(raw as Parameters<typeof receiveSourceControlResult>[0]);
          break;
        case "task_details_saved":
          receiveTaskDetailsSave(raw as Parameters<typeof receiveTaskDetailsSave>[0]);
          break;
        case "workspace_status":
          setStatus(event.message ? { message: event.message, error: Boolean(event.error) } : null);
          break;
        case "quick_open_paste":
          window.dispatchEvent(new CustomEvent("blackholes:quick-open-paste", { detail: raw }));
          break;
        case "quick_open": setQuickOpen(raw as QuickOpenState); break;
        case "quick_open_close": setQuickOpen(null); break;
        case "app_modal": setModal(event.modal || null); break;
        case "app_modal_feedback":
          setModal(current => current && current.request_id === event.request_id ? { ...current, feedback: event.feedback } : current);
          break;
      }
    } };
    postNative({ type: "ready" });
    return () => { delete window.blackholesNative; };
  }, []);

  const modalLayer = modal && <AppModal key={modal.request_id || `${modal.kind}:${modal.workspace_id || modal.task_id || modal.terminal_id}`}
    modal={modal} language={language} onDismiss={() => {
      setModal(null);
      postNative({ type: "dismiss_app_modal" });
    }} />;
  const quickOpenLayer = quickOpen && <QuickOpen state={quickOpen} />;
  if (overTerminal) return <>{modalLayer}{quickOpenLayer}</>;
  return <>
    {surface && <WorkspaceSurface event={surface} />}
    {surface?.surface !== "settings" && !modal && !quickOpen && <SidebarResizeHandle width={sidebarWidth}
      left={0} hitWidth={8} edge="left" fixed label={language === "en" ? "Resize sidebar" : "Cambiar ancho del menú lateral"} />}
    {status && <button type="button" className={`workspace-status${status.error ? " is-error" : ""}`}
      onClick={() => { setStatus(null); postNative({ type: "dismiss_status" }); }}>{status.message}</button>}
    {modalLayer}{quickOpenLayer}
  </>;
}

const root = document.querySelector<HTMLElement>("#root");
if (!root) throw new Error("Blackholes workspace root was not found.");
createRoot(root).render(<StrictMode><WorkspaceApp /></StrictMode>);

import { Rocket, X } from "lucide-react";
import { useEffect, useRef } from "react";
import { postNative } from "../shared/native";

export interface ReleaseNotesData {
  language: "en" | "es";
  version: string;
  notes: string;
}

function inline(text: string) {
  return text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).map((part, index) =>
    part.startsWith("**") ? <strong key={index}>{part.slice(2, -2)}</strong>
      : part.startsWith("`") ? <code key={index}>{part.slice(1, -1)}</code> : part);
}

function content(notes: string) {
  return notes.replace(/^# .*(?:\r?\n|$)/, "").trim().split(/\n\s*\n/).map((block, index) => {
    const text = block.replace(/\n\s+/g, " ");
    if (text.startsWith("## ")) return <h2 key={index}>{inline(text.slice(3))}</h2>;
    if (text.startsWith("- ")) return <ul key={index}>{text.split(/\n- /).map((item, i) =>
      <li key={i}>{inline(item.replace(/^- /, ""))}</li>)}</ul>;
    return <p key={index}>{inline(text.replace(/\n/g, " "))}</p>;
  });
}

export function ReleaseNotes({ data }: { data: ReleaseNotesData }) {
  const close = useRef<HTMLButtonElement>(null);
  const es = data.language === "es";
  useEffect(() => { close.current?.focus(); }, []);
  return <main className="release-notes" aria-labelledby="release-notes-title">
    <header className="release-notes-header">
      <span><Rocket size={16} aria-hidden="true" />Blackholes <b>v{data.version}</b></span>
      <button ref={close} type="button" className="release-notes-close"
        aria-label={es ? "Cerrar novedades" : "Close release notes"}
        title={es ? "Cerrar novedades" : "Close release notes"}
        onClick={() => postNative({ type: "close_release_notes" })}><X size={19} /></button>
    </header>
    <article className="release-notes-content">
      <div className="release-notes-emblem"><Rocket size={27} strokeWidth={1.6} aria-hidden="true" /></div>
      <h1 id="release-notes-title">{es ? "Novedades de Blackholes" : "What’s new in Blackholes"}</h1>
      <span className="release-notes-version">{es ? "Versión" : "Version"} {data.version}</span>
      <div className="release-notes-body">{content(data.notes)}</div>
      <p className="release-notes-hint">{es
        ? "Puedes volver a ver estas novedades desde el icono junto a la versión."
        : "You can revisit these notes using the icon beside the version."}</p>
    </article>
  </main>;
}

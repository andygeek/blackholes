import { ArrowUpRight, ChevronDown, Rocket, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { postNative } from "../shared/native";

interface Release {
  version: string;
  notes: string;
}

export interface ReleaseNotesData {
  language: "en" | "es";
  version: string;
  releases: Release[];
}

const historyUrl = "https://github.com/andygeek/blackholes/releases";

function inline(text: string) {
  return text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).map((part, index) =>
    part.startsWith("**") ? <strong key={index}>{part.slice(2, -2)}</strong>
      : part.startsWith("`") ? <code key={index}>{part.slice(1, -1)}</code> : part);
}

function releaseContent(notes: string) {
  const blocks = notes.replace(/^# .*(?:\r?\n|$)/, "").trim().split(/\n\s*\n/);
  const summary = (blocks.shift() || "").replace(/\s*\n\s*/g, " ");
  // Installation and release verification remain in the full online notes.
  const changes = blocks.join("\n\n").split(/^## (?:Install or update|Instalar o actualizar|Validation and limitations)\s*$/m)[0];
  return { summary, changes: changes.trim() };
}

function content(notes: string) {
  return notes.split(/\n\s*\n/).map((block, index) => {
    const text = block.replace(/\n[ \t]+/g, " ");
    if (text.startsWith("## ")) return <h3 key={index}>{inline(text.slice(3))}</h3>;
    if (text.startsWith("- ")) return <ul key={index}>{text.split(/\n- /).map((item, i) =>
      <li key={i}>{inline(item.replace(/^- /, ""))}</li>)}</ul>;
    return <p key={index}>{inline(text.replace(/\n/g, " "))}</p>;
  });
}

function ReleaseCard({ release, current, es }: { release: Release; current: boolean; es: boolean }) {
  const [expanded, setExpanded] = useState(current);
  const { summary, changes } = releaseContent(release.notes);
  const bodyId = `release-${release.version}`;
  return <li className={`release-card${current ? " is-current" : ""}`}>
    <article aria-labelledby={`${bodyId}-title`}>
      <div className="release-card-heading">
        <h2 id={`${bodyId}-title`}>v{release.version}</h2>
        {current && <span className="release-current-badge">{es ? "Versión actual" : "Current version"}</span>}
      </div>
      <p className="release-card-summary">{inline(summary)}</p>
      <button type="button" className="release-card-toggle" aria-expanded={expanded} aria-controls={bodyId}
        onClick={() => setExpanded(value => !value)}>
        {expanded ? (es ? "Ocultar cambios" : "Hide changes") : (es ? "Ver cambios" : "View changes")}
        <ChevronDown size={14} aria-hidden="true" />
      </button>
      <div id={bodyId} className="release-notes-body" hidden={!expanded}>{content(changes)}</div>
    </article>
  </li>;
}

export function ReleaseNotes({ data }: { data: ReleaseNotesData }) {
  const close = useRef<HTMLButtonElement>(null);
  const es = data.language === "es";
  useEffect(() => { close.current?.focus(); }, []);
  return <main className="release-notes" aria-labelledby="release-notes-title">
    <header className="release-notes-header">
      <span><Rocket size={15} aria-hidden="true" />{es ? "Novedades" : "What’s new"}</span>
      <button ref={close} type="button" className="release-notes-close"
        aria-label={es ? "Cerrar novedades" : "Close release notes"}
        title={es ? "Cerrar novedades" : "Close release notes"}
        onClick={() => postNative({ type: "close_release_notes" })}><X size={18} /></button>
    </header>
    <div className="release-notes-content">
      <div className="release-notes-intro">
        <div className="release-notes-emblem"><Rocket size={23} strokeWidth={1.6} aria-hidden="true" /></div>
        <h1 id="release-notes-title">{es ? "Cada versión, un poco mejor." : "A little better with every release."}</h1>
        <p>{es ? "Descubre los últimos cambios de Blackholes." : "Catch up on the latest changes in Blackholes."}</p>
        <a className="release-history-link" href={historyUrl}
          onClick={event => { event.preventDefault(); postNative({ type: "open_url", url: historyUrl }); }}>
          {es ? "Ver todas las versiones en GitHub" : "All releases on GitHub"}<ArrowUpRight size={14} aria-hidden="true" />
        </a>
      </div>
      <ol className="release-history" aria-label={es ? "Últimas versiones" : "Recent releases"}>
        {data.releases.map(release => <ReleaseCard key={release.version} release={release}
          current={release.version === data.version} es={es} />)}
      </ol>
      <p className="release-notes-hint">{es
        ? "Vuelve a estas novedades desde el cohete junto a la versión."
        : "Revisit these notes using the rocket beside the version."}</p>
    </div>
  </main>;
}

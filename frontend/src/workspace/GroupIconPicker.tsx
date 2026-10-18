import { useEffect, useRef, useState } from "react";
import { GroupIcon, groupIcons } from "../shared/GroupIcon";

const emojiChoices = [
  "😀", "😎", "🤖", "👾", "👻", "👽", "🧠", "💡",
  "🚀", "🛸", "🛰️", "🌍", "🌙", "⭐", "☀️", "🌈",
  "⚡", "🔥", "💧", "🌱", "🌳", "🌵", "🌻", "🍀",
  "🐱", "🐶", "🦊", "🐻", "🐼", "🐸", "🦄", "🐙",
  "🎯", "🎨", "🎮", "🎵", "🎬", "📸", "🎉", "🏆",
  "💻", "📱", "🔧", "⚙️", "🔬", "🧪", "🧩", "📦",
  "📚", "📝", "📌", "🔖", "💬", "🔗", "🔒", "🔑",
  "❤️", "💜", "💙", "✅", "🚧", "🛠️", "💎", "🍎",
];

export function GroupIconPicker({ value, language, onChange }: { value: string; language: "en" | "es"; onChange(value: string): void }) {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<"emoji" | "icons">("emoji");
  const [custom, setCustom] = useState("");
  const [error, setError] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const t = (en: string, es: string) => language === "en" ? en : es;
  const choose = (icon: string) => { onChange(icon); setOpen(false); setError(false); trigger.current?.focus(); };
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!container.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener("pointerdown", outside, true);
    return () => document.removeEventListener("pointerdown", outside, true);
  }, [open]);
  const chooseCustom = () => {
    const input = custom.trim();
    const symbol = typeof Intl.Segmenter === "function" ? [...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(input)][0]?.segment : Array.from(input)[0];
    if (!symbol || !/[^\x00-\x7F]/.test(symbol) || /\s/.test(symbol)) { setError(true); return; }
    choose(`emoji:${symbol}`);
  };
  return <div className="group-icon-picker" ref={container} onKeyDown={event => {
    if (event.key === "Escape" && open) { event.preventDefault(); event.stopPropagation(); setOpen(false); trigger.current?.focus(); }
  }}>
    <button ref={trigger} className="group-page-icon" type="button" title={t("Change icon", "Cambiar icono")} aria-label={t("Change group icon", "Cambiar icono del grupo")} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(!open)}><GroupIcon value={value} size={52} /></button>
    {open && <div className="group-icon-popover" role="dialog" aria-label={t("Choose an icon", "Elegir un icono")}>
      <div className="group-icon-tabs" role="group" aria-label={t("Icon style", "Estilo de icono")}>
        <button type="button" aria-pressed={tab === "emoji"} onClick={() => setTab("emoji")}>Emoji</button>
        <button type="button" aria-pressed={tab === "icons"} onClick={() => setTab("icons")}>{t("Icons", "Iconos")}</button>
      </div>
      <div className="group-icon-grid" role="group" aria-label={t("Available icons", "Iconos disponibles")}>
        {tab === "emoji" ? emojiChoices.map(emoji => <button type="button" key={emoji} aria-label={emoji} aria-pressed={value === `emoji:${emoji}`} onClick={() => choose(`emoji:${emoji}`)}>{emoji}</button>) : groupIcons.map(icon => <button type="button" key={icon.value} aria-label={language === "en" ? icon.en : icon.es} title={language === "en" ? icon.en : icon.es} aria-pressed={value === icon.value} onClick={() => choose(icon.value)}><icon.Icon size={22} /></button>)}
      </div>
      {tab === "emoji" && <div className="group-custom-emoji"><input aria-label={t("Paste an emoji", "Pega un emoji")} placeholder={t("Paste an emoji…", "Pega un emoji…")} maxLength={64} value={custom} onChange={event => { setCustom(event.target.value); setError(false); }} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); chooseCustom(); } }} /><button type="button" onClick={chooseCustom}>{t("Use", "Usar")}</button></div>}
      {error && <p role="alert">{t("Paste an emoji or symbol.", "Pega un emoji o símbolo.")}</p>}
      <button type="button" className="group-icon-reset" onClick={() => choose("layers")}>{t("Default icon", "Icono predeterminado")}</button>
    </div>}
  </div>;
}

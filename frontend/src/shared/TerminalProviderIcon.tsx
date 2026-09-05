import { Gem, SquareTerminal } from "lucide-react";
import openai from "../../../assets/icons/codex.svg?raw";
import claude from "../../../assets/icons/claude-code.svg?raw";

/** Trusted, bundled provider artwork; never terminal-supplied SVG/HTML. */
export function TerminalProviderIcon({ provider }: { provider: string }) {
  const artwork = provider === "codex" ? openai : provider === "claude" ? claude : null;
  return artwork
    ? <span className={`terminal-provider terminal-provider--${provider}`} aria-hidden="true" dangerouslySetInnerHTML={{ __html: artwork }} />
    : <span className={`terminal-provider terminal-provider--${provider}`} aria-hidden="true">{provider === "gemini" ? <Gem size={15} /> : <SquareTerminal size={15} />}</span>;
}

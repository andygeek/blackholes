import { Gem, SquareTerminal } from "lucide-react";
import openai from "../../../assets/icons/codex.svg?raw";
import claude from "../../../assets/icons/claude-code.svg?raw";
import opencode from "../../../assets/icons/opencode.svg?raw";
import opencodeDark from "../../../assets/icons/opencode-dark.svg?raw";
import antigravity from "../../../assets/icons/antigravity.png?inline";

/** Trusted, bundled provider artwork; never terminal-supplied SVG/HTML. */
export function TerminalProviderIcon({ provider }: { provider: string }) {
  if (provider === "antigravity") {
    return <span className="terminal-provider terminal-provider--antigravity" aria-hidden="true">
      <img src={antigravity} alt="" />
    </span>;
  }
  if (provider === "opencode") {
    return <span className="terminal-provider terminal-provider--opencode" aria-hidden="true">
      <span className="provider-logo-light" dangerouslySetInnerHTML={{ __html: opencode }} />
      <span className="provider-logo-dark" dangerouslySetInnerHTML={{ __html: opencodeDark }} />
    </span>;
  }
  const artwork = provider === "codex" ? openai : provider === "claude" ? claude : null;
  return artwork
    ? <span className={`terminal-provider terminal-provider--${provider}`} aria-hidden="true" dangerouslySetInnerHTML={{ __html: artwork }} />
    : <span className={`terminal-provider terminal-provider--${provider}`} aria-hidden="true">{
      provider === "gemini" ? <Gem size={15} />
        : <SquareTerminal size={15} />
    }</span>;
}

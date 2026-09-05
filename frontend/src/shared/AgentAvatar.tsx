import { useId } from "react";
import { useAvatarPersonality } from "./useAvatarPersonality";

export type AgentIdentity = "mercury" | "earthy" | "saturny";

const identityNames: Record<AgentIdentity, string> = {
  mercury: "Mercury",
  earthy: "Earthy",
  saturny: "Saturny",
};

const legacyIdentities: Record<string, AgentIdentity> = {
  orange: "mercury",
  coral: "mercury",
  peach: "mercury",
  sage: "earthy",
  mint: "earthy",
  sky: "earthy",
  amber: "saturny",
  lavender: "saturny",
  rose: "saturny",
};

export const normalizeIdentity = (value?: string): AgentIdentity => {
  if (value === "mercury" || value === "earthy" || value === "saturny") return value;
  return legacyIdentities[value || ""] || "mercury";
};

export const agentIdentityName = (value?: string): string => (
  identityNames[normalizeIdentity(value)]
);

interface AgentAvatarProps {
  identity?: string;
  size?: number;
  busy?: boolean;
}

const Eyes = ({ y = 42 }: { y?: number }) => (
  <g className="agent-avatar__eyes">
    <rect className="agent-avatar__eye agent-avatar__eye--left" x="38" y={y} width="7" height="19" rx="3.5" />
    <rect className="agent-avatar__eye agent-avatar__eye--right" x="56" y={y} width="7" height="19" rx="3.5" />
    <g className="agent-avatar__closed-eyes" fill="none" stroke="#10141c" strokeWidth="2.6" strokeLinecap="round">
      <path d={`M38 ${y + 11} q3.5 4 7 0`} />
      <path d={`M56 ${y + 11} q3.5 4 7 0`} />
    </g>
  </g>
);

export function AgentAvatar({ identity: value, size = 32, busy }: AgentAvatarProps) {
  const identity = normalizeIdentity(value);
  const animationRef = useAvatarPersonality(identity, busy);
  const generatedId = useId().replaceAll(":", "");
  const gradientId = `${identity}-gradient-${generatedId}`;
  const clipId = `${identity}-clip-${generatedId}`;

  return (
    <svg
      ref={animationRef}
      className={`agent-avatar-svg agent-avatar-svg--${identity}`}
      viewBox="0 0 100 100"
      width={size}
      height={size}
      aria-hidden="true"
      focusable="false"
      style={{ "--avatar-size": `${size}px` } as React.CSSProperties}
    >
      <defs>
        <radialGradient id={gradientId} cx="34%" cy="25%" r="76%">
          <stop offset="0%" stopColor={identity === "mercury" ? "#d4d3d7" : identity === "earthy" ? "#62b9ff" : "#ffe1a1"} />
          <stop offset="100%" stopColor={identity === "mercury" ? "#777781" : identity === "earthy" ? "#2077eb" : "#c58a42"} />
        </radialGradient>
        {identity !== "mercury" && (
          <clipPath id={clipId}>
            <circle cx="50" cy={identity === "saturny" ? 49 : 50} r={identity === "saturny" ? 31 : 42} />
          </clipPath>
        )}
      </defs>

      <ellipse className="agent-avatar__shadow" cx="50" cy="94" rx="23" ry="3" />
      <g className="agent-avatar__body">

      {identity === "mercury" && (
        <>
          <circle cx="50" cy="50" r="42" fill={`url(#${gradientId})`} />
          {[
            [27, 30, 9, 0.16], [66, 24, 4, 0.17], [78, 42, 7, 0.2],
            [25, 61, 6, 0.18], [68, 70, 12, 0.16], [44, 22, 3, 0.14],
            [39, 77, 5, 0.18], [82, 65, 3, 0.18], [19, 45, 3, 0.2],
          ].map(([cx, cy, radius, opacity]) => (
            <circle key={`${cx}-${cy}`} cx={cx} cy={cy} r={radius} fill={`rgba(67, 67, 78, ${opacity})`} />
          ))}
          <path d="M20 69c15 18 43 24 64 5-9 13-22 20-36 20-13 0-23-4-28-9Z" fill="rgba(255,255,255,.08)" />
          <Eyes />
        </>
      )}

      {identity === "earthy" && (
        <>
          <circle cx="50" cy="50" r="42" fill={`url(#${gradientId})`} />
          <g clipPath={`url(#${clipId})`} fill="#6ac84c">
            <path d="M9 20c10-13 24-19 39-20 1 8-3 13-11 15-7 2-8 8-12 14-4 7-10 5-17 6Z" />
            <path d="M72 3c16 6 27 18 31 34-8 2-13-2-17-7-3-4-12-4-15-11-2-6 0-11 1-16Z" />
            <path d="M4 65c9-6 19-4 25 4 5 7 11 8 18 8 8 0 13 7 13 16-22 5-44-5-56-28Z" />
            <path d="M78 66c7-2 15-8 25-4-3 18-14 31-30 37-4-8-6-17 1-23 3-3 1-8 4-10Z" />
          </g>
          <circle cx="50" cy="50" r="42" fill="none" stroke="rgba(255,255,255,.14)" strokeWidth="1.2" />
          <Eyes />
        </>
      )}

      {identity === "saturny" && (
        <>
          <ellipse cx="50" cy="54" rx="48" ry="16" fill="none" stroke="#d5aa70" strokeWidth="9" transform="rotate(-13 50 54)" />
          <ellipse cx="50" cy="54" rx="47" ry="14" fill="none" stroke="rgba(255,236,193,.65)" strokeWidth="2" transform="rotate(-13 50 54)" />
          <circle cx="50" cy="49" r="31" fill={`url(#${gradientId})`} />
          <g clipPath={`url(#${clipId})`}>
            <rect x="17" y="27" width="66" height="5" fill="rgba(255,248,214,.38)" />
            <rect x="17" y="37" width="66" height="4" fill="rgba(163,104,47,.12)" />
            <rect x="17" y="55" width="66" height="5" fill="rgba(255,245,205,.28)" />
            <rect x="17" y="67" width="66" height="4" fill="rgba(153,91,42,.14)" />
          </g>
          <path d="M5 60c17 17 59 20 88-1" fill="none" stroke="#e5bd82" strokeWidth="9" strokeLinecap="round" />
          <path d="M7 58c20 15 58 18 84-1" fill="none" stroke="rgba(255,239,202,.72)" strokeWidth="2" />
          <Eyes y={39} />
        </>
      )}
      <g className="agent-avatar__sleep-bubble" pointerEvents="none">
        <path d="M52 58 Q55 62 59 64" fill="none" stroke="#bce2ee" strokeWidth="1.6" />
        <ellipse cx="66" cy="70" rx="10" ry="12" fill="rgba(201,237,249,.7)" stroke="#a9d5e6" strokeWidth="1.2" />
        <path d="M64 61 Q59 62 59 68" fill="none" stroke="rgba(255,255,255,.95)" strokeWidth="2.5" strokeLinecap="round" />
      </g>
      </g>
    </svg>
  );
}

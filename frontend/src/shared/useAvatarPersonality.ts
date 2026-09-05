import { useEffect, useRef } from "react";
import type { AgentIdentity } from "./AgentAvatar";

type Gesture = "glance" | "look-up" | "side-eye" | "blink" | "double-blink" | "wink" | "curious" | "hop" | "bounce" | "wake";

const personalities: Record<AgentIdentity, { pause: number; gestures: Gesture[] }> = {
  mercury: { pause: 4800, gestures: ["glance", "side-eye", "look-up", "blink", "double-blink", "curious", "hop"] },
  earthy: { pause: 3000, gestures: ["glance", "side-eye", "look-up", "blink", "wink", "hop", "bounce", "double-blink"] },
  saturny: { pause: 4000, gestures: ["glance", "side-eye", "look-up", "curious", "wink", "bounce", "blink"] },
};

// Gestures update only SVG attributes, not React/chat state. Offscreen avatars
// have no active timers; every visible instance has its own rhythm and rest periods.
export function useAvatarPersonality(identity: AgentIdentity, busy?: boolean) {
  const ref = useRef<SVGSVGElement>(null);
  useEffect(() => {
    const avatar = ref.current;
    if (!avatar) return;
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    const personality = personalities[identity];
    // Only real agents with a known idle state sleep; previews/delegation snapshots
    // must not imply that an agent with unknown runtime status has stopped working.
    const sleepAfter = (identity === "mercury" ? 45000 : identity === "saturny" ? 55000 : 65000) + Math.random() * 10000;
    let idleSince = performance.now();
    let visible = false;
    let disposed = false;
    let timer: number | undefined;
    const canAnimate = () => !disposed && visible && !document.hidden && !preference.matches;
    const rest = () => {
      window.clearTimeout(timer);
      timer = undefined;
      avatar.removeAttribute("data-gesture");
      avatar.removeAttribute("data-rest");
      avatar.style.setProperty("--gaze-x", "0px");
      avatar.style.setProperty("--gaze-y", "0px");
    };
    const schedule = (initial = false) => {
      if (!canAnimate()) return;
      const idleTime = performance.now() - idleSince;
      if (busy === false && idleTime >= sleepAfter - 9000) {
        rest();
        avatar.dataset.rest = idleTime >= sleepAfter ? "sleeping" : "drowsy";
        if (idleTime < sleepAfter) timer = window.setTimeout(() => schedule(), sleepAfter - idleTime);
        return;
      }
      timer = window.setTimeout(() => {
        const gesture = personality.gestures[Math.floor(Math.random() * personality.gestures.length)];
        play(gesture);
      }, initial ? 800 + Math.random() * 4200 : personality.pause + Math.random() * 5000);
    };
    const play = (gesture: Gesture) => {
      if (!canAnimate()) return;
      rest();
      avatar.dataset.gesture = gesture;
      avatar.style.setProperty("--hop-tilt", `${Math.random() > .5 ? 7 : -7}deg`);
      avatar.style.setProperty("--wink-left", Math.random() > .5 ? "1" : "0.12");
      avatar.style.setProperty("--wink-right", avatar.style.getPropertyValue("--wink-left") === "1" ? "0.12" : "1");
      if (gesture === "glance" || gesture === "curious") {
        const angle = Math.random() * Math.PI * 2;
        avatar.style.setProperty("--gaze-x", `${Math.cos(angle) * 4.5}px`);
        avatar.style.setProperty("--gaze-y", `${Math.sin(angle) * 3}px`);
      } else if (gesture === "side-eye") {
        avatar.style.setProperty("--gaze-x", `${Math.random() > .5 ? 5.5 : -5.5}px`);
        avatar.style.setProperty("--gaze-y", "1px");
      } else if (gesture === "look-up") {
        avatar.style.setProperty("--gaze-x", `${Math.random() * 4 - 2}px`);
        avatar.style.setProperty("--gaze-y", "-5px");
      }
      timer = window.setTimeout(() => {
        rest();
        schedule();
      }, gesture === "curious" ? 2600 : ["glance", "side-eye", "look-up"].includes(gesture) ? 1800 + Math.random() * 900 : 1300);
    };
    const refresh = () => {
      rest();
      schedule(true);
    };
    const greet = () => {
      idleSince = performance.now();
      if (avatar.dataset.rest) play("wake");
      // Don't restart a jump on every pointer movement across its animated bounds.
      else if (!avatar.dataset.gesture) play(identity === "earthy" ? "hop" : "wink");
    };
    const observer = new IntersectionObserver(([entry]) => {
      if (visible === entry.isIntersecting) return;
      visible = entry.isIntersecting;
      refresh();
    });
    observer.observe(avatar);
    document.addEventListener("visibilitychange", refresh);
    preference.addEventListener("change", refresh);
    const interactionTarget = avatar.closest("button, [role='button']") || avatar;
    interactionTarget.addEventListener("pointerenter", greet);
    interactionTarget.addEventListener("pointerdown", greet);
    interactionTarget.addEventListener("focusin", greet);
    return () => {
      disposed = true;
      rest();
      observer.disconnect();
      document.removeEventListener("visibilitychange", refresh);
      preference.removeEventListener("change", refresh);
      interactionTarget.removeEventListener("pointerenter", greet);
      interactionTarget.removeEventListener("pointerdown", greet);
      interactionTarget.removeEventListener("focusin", greet);
    };
  }, [identity, busy]);
  return ref;
}

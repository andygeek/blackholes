export type NativeCommand = Record<string, unknown> & { type: string };

export const postNative = (command: NativeCommand): void => {
  window.ipc?.postMessage(JSON.stringify(command));
};

export const createId = (): string => {
  if (typeof globalThis.crypto?.randomUUID === "function") {
    return globalThis.crypto.randomUUID();
  }
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (character) => {
    const random = Math.floor(Math.random() * 16);
    const value = character === "x" ? random : (random & 0x3) | 0x8;
    return value.toString(16);
  });
};

declare global {
  interface Window {
    ipc?: {
      postMessage(message: string): void;
    };
    blackholesNative?: {
      receive(event: unknown): void;
    };
    blackholesNavigation?: {
      receive(event: unknown): void;
    };
  }
}

export {};

import { createInterface } from "node:readline";

const errorMessage = (error) => {
  if (!error) return "JSON-RPC request failed.";
  if (typeof error === "string") return error;
  return error.message || JSON.stringify(error);
};

export class JsonRpcProcess {
  constructor(child, { onNotification, onRequest, onDiagnostic, onClose } = {}) {
    this.child = child;
    this.onNotification = onNotification;
    this.onRequest = onRequest;
    this.onDiagnostic = onDiagnostic;
    this.onClose = onClose;
    this.nextId = 1;
    this.pending = new Map();
    this.closed = false;
    this.lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
    this.lines.on("line", (line) => this.#receive(line));
    child.stderr.on("data", (chunk) => {
      const message = chunk.toString().trim();
      if (message) this.onDiagnostic?.(message);
    });
    child.once("error", (error) => this.#close(error));
    child.once("close", (code, signal) => {
      const suffix = signal ? ` (${signal})` : "";
      this.#close(code === 0 || code === null
        ? null
        : new Error(`JSON-RPC process exited with code ${code}${suffix}.`));
    });
  }

  request(method, params = {}) {
    if (this.closed) return Promise.reject(new Error("JSON-RPC process is closed."));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.#write({ jsonrpc: "2.0", id, method, params });
    });
  }

  notify(method, params = {}) {
    if (this.closed) return;
    this.#write({ jsonrpc: "2.0", method, params });
  }

  stop() {
    if (this.closed) return;
    this.child.kill("SIGTERM");
  }

  #write(message) {
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  #receive(line) {
    if (!line.trim()) return;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      this.onDiagnostic?.(line);
      return;
    }

    if (Object.hasOwn(message, "id") && (Object.hasOwn(message, "result") || Object.hasOwn(message, "error"))) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) {
        const error = new Error(errorMessage(message.error));
        error.code = message.error.code;
        error.data = message.error.data;
        pending.reject(error);
      } else {
        pending.resolve(message.result);
      }
      return;
    }

    if (Object.hasOwn(message, "id") && message.method) {
      Promise.resolve(this.onRequest?.(message.method, message.params || {}))
        .then((result) => this.#write({ jsonrpc: "2.0", id: message.id, result: result ?? {} }))
        .catch((error) => this.#write({
          jsonrpc: "2.0",
          id: message.id,
          error: { code: -32603, message: errorMessage(error) },
        }));
      return;
    }

    if (message.method) this.onNotification?.(message.method, message.params || {});
  }

  #close(error) {
    if (this.closed) return;
    this.closed = true;
    this.lines.close();
    const reason = error || new Error("JSON-RPC process closed before completing the request.");
    for (const pending of this.pending.values()) pending.reject(reason);
    this.pending.clear();
    this.onClose?.(reason);
  }
}

// A small MCP client for tests: starts the plugin's command in the plugin folder, with no credential variable, and
// sends JSON-RPC requests one line at a time.
import { spawn } from "node:child_process";
import path from "node:path";
import readline from "node:readline";

/** Starts `command` (a path inside `folder`) the way Nexus Desktop's launcher does. */
export function mcp(folder, command, env = {}) {
  const child = spawn(path.join(folder, command), [], {
    cwd: folder,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, ...env },
    stdio: ["pipe", "pipe", "inherit"],
  });
  const waiting = new Map();
  let next = 1;
  readline.createInterface({ input: child.stdout }).on("line", (line) => {
    const message = JSON.parse(line);
    const pending = waiting.get(message.id);
    if (!pending) return;
    waiting.delete(message.id);
    if (message.error) pending.reject(new Error(message.error.message));
    else pending.resolve(message.result);
  });
  const write = (message) => child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`);
  return {
    request(method, params) {
      const id = next++;
      write({ id, method, params });
      return new Promise((resolve, reject) => {
        waiting.set(id, { resolve, reject });
        setTimeout(() => waiting.has(id) && reject(new Error(`${method}: no answer in 5 s`)), 5000).unref();
      });
    },
    notify(method, params) {
      write({ method, params });
    },
    close() {
      child.stdin.end();
      child.kill();
    },
  };
}

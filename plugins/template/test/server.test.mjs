// Starts the plugin as Nexus Desktop's launcher does, in its own folder, lists its tools and calls one.
// Keep this test when you add tools: it is the quickest proof that the plugin starts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { mcp } from "./mcp-client.mjs";

const folder = fileURLToPath(new URL("..", import.meta.url));
const manifest = JSON.parse(readFileSync(path.join(folder, "nexus-plugin.json"), "utf8"));

test("the manifest names a program inside the folder", () => {
  assert.equal(manifest.apiVersion, 0);
  assert.match(manifest.id, /^[a-z0-9-]{3,40}$/);
  assert.ok(manifest.mcp.command.startsWith("bin/"), "the program is inside the plugin folder");
});

test("the server starts, lists its tools and answers a call", async () => {
  const server = mcp(folder, manifest.mcp.command);
  try {
    const init = await server.request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "0" } });
    assert.equal(init.serverInfo.name, manifest.id);
    server.notify("notifications/initialized");
    const { tools } = await server.request("tools/list");
    assert.ok(tools.length > 0, "at least one tool");
    const call = await server.request("tools/call", { name: "echo", arguments: { text: "ping" } });
    assert.equal(call.content[0].text, "ping");
  } finally {
    server.close();
  }
});

// Starts the plugin as Nexus Desktop's launcher does, in its own folder, and talks MCP to it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { mcp } from "./mcp-client.mjs";

const folder = fileURLToPath(new URL("..", import.meta.url));
const manifest = JSON.parse(readFileSync(new URL("../nexus-plugin.json", import.meta.url), "utf8"));

test("the manifest asks for nothing", () => {
  assert.equal(manifest.apiVersion, 0);
  assert.equal(manifest.id, "hello-nexus");
  assert.equal(manifest.secrets, undefined);
  assert.equal(manifest.permissions, undefined);
});

test("the server lists hello and answers a call", async () => {
  const server = mcp(folder, manifest.mcp.command);
  try {
    const init = await server.request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "0" } });
    assert.equal(init.serverInfo.name, "hello-nexus");
    server.notify("notifications/initialized");
    const { tools } = await server.request("tools/list");
    assert.deepEqual(tools.map((tool) => tool.name), ["hello"]);
    const call = await server.request("tools/call", { name: "hello", arguments: { name: "Ada" } });
    assert.equal(call.content[0].text, "Hello, Ada! This answer comes from the hello-nexus plugin.");
    const unknown = await server.request("tools/call", { name: "nope", arguments: {} });
    assert.equal(unknown.isError, true);
  } finally {
    server.close();
  }
});

# hello-nexus

The smallest Nexus Desktop plugin: one MCP tool, `hello`, and no permissions or secrets. Use it to check that a plugin reaches your Code sessions, from install to a tool call.

## Test it without the app

```bash
node --test test/*.test.mjs
```

The test starts the server in this folder, as the app's launcher does, lists its tools and calls `hello`.

## Try it in Nexus Desktop

Plugins are off by default. They need a Nexus Desktop build with plugin API version 0, which is not in a release yet. Until Settings › Plugins ships, install by hand on macOS:

1. Quit Nexus Desktop.
2. Copy this folder to the app's plugins folder:

   ```bash
   mkdir -p "$HOME/Library/Application Support/xyz.nexus.desktop/plugins"
   cp -R . "$HOME/Library/Application Support/xyz.nexus.desktop/plugins/hello-nexus"
   ```

3. Approve it in `$HOME/Library/Application Support/xyz.nexus.desktop/settings.json`. Add this key at the top level, next to the others:

   ```json
   "plugins": {
     "hello-nexus": { "version": "0.1.0", "grant": { "mcp": { "command": "bin/hello-nexus" } } }
   }
   ```

   The grant must match what the manifest asks for. If it does not, the plugin waits for a new approval.

4. Turn plugins on, then open the app:

   ```bash
   launchctl setenv NEXUS_DESKTOP_PLUGINS 1
   ```

5. Start a Code session and ask the agent: "Use the hello tool to greet Ada." It answers: "Hello, Ada! This answer comes from the hello-nexus plugin."

To turn plugins off again, run `launchctl unsetenv NEXUS_DESKTOP_PLUGINS` and reopen the app.

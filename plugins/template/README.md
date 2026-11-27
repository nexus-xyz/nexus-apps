# create-nexus-plugin

This folder is the template for a Nexus Desktop plugin. Copy it to start your own. It holds a manifest, a small MCP server on stdio with one tool, and a test that starts the server and calls the tool. It needs Node 18 or later and no packages.

## Make your plugin

1. Copy this folder and give it your plugin's id as its name, for example `acme-notes`.
2. Rename `bin/my-plugin` to `bin/<your id>`. Keep it executable.
3. In `nexus-plugin.json`, set `id`, `name`, `description`, `author` and `mcp.command`. The manifest rules are in `../../docs/plugins/manifest.md`.
4. In `bin/<your id>`, set `SERVER.name` to your id. Replace the `echo` tool in `TOOLS` with your own. A tool's `run` may be async.
5. Declare what the plugin uses: each secret in `secrets`, and each host, path and program in `permissions`. The person sees this list before the plugin runs.
6. Run the test from this folder:

   ```bash
   node --test test/*.test.mjs
   ```

   Change the test's call from `echo` to one of your tools.

## Secrets

A secret is a value the person enters in the app, such as an API token. Name it in `secrets`, for example `"secrets": ["ACME_TOKEN"]`. The plugin reads it as `process.env.ACME_TOKEN`. Never hard-code a token in the plugin.

The plugin gets only the secrets it declares. It gets no Nexus token and no model provider key, so a plugin cannot spend the person's credits.

## Try it in Nexus Desktop

Follow the steps in `../examples/hello-nexus/README.md`, with your folder in place of `hello-nexus`.

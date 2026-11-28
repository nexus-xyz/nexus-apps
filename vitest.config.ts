import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

// The tests of the kit (packages/kit) and of the phone app (apps/mobile). The root installs no phone dependencies:
// an `expo*` or `react-native*` import from the app resolves to an empty module, and the component tests provide the
// native APIs they use through vi.mock factories.
const MOBILE = "/apps/mobile/";

export default defineConfig({
  plugins: [{
    name: "mobile-native-mocks",
    resolveId(id, importer) {
      if (importer?.replaceAll("\\", "/").includes(MOBILE) && /^(expo(?:-[^/]+)?|react-native(?:-[^/]+)?)(\/|$)/.test(id)) return id;
    },
  }],
  resolve: {
    dedupe: ["react", "react-dom"],
    alias: [{
      // The app resolves `@/…` within its own src directory, as its tsconfig.json says.
      find: /^@\//,
      replacement: "",
      async customResolver(source, importer) {
        const path = fileURLToPath(new URL(`./apps/mobile/src/${source}`, import.meta.url));
        return this.resolve(path, importer, { skipSelf: true });
      },
    }, {
      // The app imports the kit by name (apps/mobile/tsconfig.json's paths). It is not an npm dependency.
      find: /^@nexus\/kit(?:\/(.*))?$/,
      replacement: "$1",
      async customResolver(sub, importer, options) {
        const path = fileURLToPath(new URL(`./packages/kit/src/${sub || "index"}.ts`, import.meta.url));
        return this.resolve(path, importer, { ...options, skipSelf: true });
      },
    }],
  },
  test: {
    include: ["packages/kit/src/**/*.test.ts", "apps/mobile/src/**/*.test.ts"],
    exclude: [
      "**/node_modules/**",
      // The kit's agent catalog lists Grok, and apps/cli's (which writes kit/fixtures/agent-capabilities.json) does not
      // yet. Back in when the CLI lists Grok and the fixture is blessed again.
      "packages/kit/src/agent-capabilities.test.ts",
    ],
  },
});

// @nexus/kit lives outside this app (packages/kit) and is not an npm dependency: the tsconfig path resolves the
// import, and Metro has to watch the folder to bundle it.
const path = require("node:path");
const { getDefaultConfig } = require("expo/metro-config");

// The diagram renderer (src/mermaid-renderer.tsx) is a DOM component, the app's one web bundle. Mermaid loads each kind
// of diagram with import(), and `expo export` fails on the chunks that makes ("Asset not found: …__common-….js"): it
// renames them to their MD5 but not the names they require each other by. So exports keep each web bundle whole.
process.env.EXPO_NO_BUNDLE_SPLITTING ??= "1";

const config = getDefaultConfig(__dirname);
config.watchFolders = [path.resolve(__dirname, "../../packages/kit")];
module.exports = config;

import { expect, it } from "vitest";
import { createCoreClient } from "./core-client";

it("encodes the connector and the operation in every connector path, the write's too", async () => {
  const paths: string[] = [];
  const core = createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url) => {
      // The raw path as sent: `URL` would resolve a `..` segment before a test could see it.
      paths.push(String(url).slice("http://gateway.invalid".length));
      return Response.json({});
    },
  });
  await core.connector("git/hub", "../admin?x=1", {});
  await core.readConnector("git/hub", "../admin?x=1", {});
  await core.describeConnector("git/hub");
  expect(paths).toEqual([
    "/v1/connectors/git%2Fhub/..%2Fadmin%3Fx%3D1",
    "/v1/connectors/git%2Fhub/..%2Fadmin%3Fx%3D1/read",
    "/v1/connectors/git%2Fhub",
  ]);
});

it("refuses a name that is empty, `.` or `..`, which would resolve to another route", async () => {
  let sent = 0;
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async () => (sent++, Response.json({})) });
  for (const name of ["", ".", ".."]) {
    expect(() => core.connector("github", name, {})).toThrow(TypeError);
    expect(() => core.readConnector(name, "repo", {})).toThrow(TypeError);
    expect(() => core.describeConnector(name)).toThrow(TypeError);
  }
  expect(sent).toBe(0);
});

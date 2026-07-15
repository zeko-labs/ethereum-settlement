import { describe, expect, it } from "vitest";
import { parseRuntimeConfig } from "./runtime";
import { runtimeConfig } from "../test/fixtures";

describe("runtime configuration", () => {
  it("accepts the versioned public explorer configuration", () => {
    expect(parseRuntimeConfig(runtimeConfig)).toEqual(runtimeConfig);
  });

  it("rejects unsafe URLs and unreasonable polling", () => {
    expect(() =>
      parseRuntimeConfig({ ...runtimeConfig, gatewayUrl: "file:///secret" }),
    ).toThrow(/http or https/);
    expect(() =>
      parseRuntimeConfig({ ...runtimeConfig, pollIntervalMs: 20 }),
    ).toThrow(/between 1000 and 60000/);
    expect(() =>
      parseRuntimeConfig({ ...runtimeConfig, schemaVersion: 2 }),
    ).toThrow(/schemaVersion/);
  });
});

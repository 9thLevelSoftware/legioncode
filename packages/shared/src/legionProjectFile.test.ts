import * as Schema from "effect/Schema";
import { describe, expect, it } from "vite-plus/test";

import { parseLegionProjectFile, LegionProjectFileFromJson } from "./legionProjectFile.ts";

const decodeJson = Schema.decodeUnknownSync(LegionProjectFileFromJson);

describe("LegionProjectFileFromJson", () => {
  it("decodes lenient JSONC with comments and trailing commas", () => {
    const decoded = decodeJson(`{
      // team scripts
      "iconPath": "assets/logo.svg",
      "scripts": [
        { "name": "Dev", "command": "pnpm dev", "stopOnSettle": true, },
      ],
    }`);

    expect(decoded.iconPath).toBe("assets/logo.svg");
    expect(decoded.scripts?.[0]).toEqual({ name: "Dev", command: "pnpm dev", stopOnSettle: true });
  });
});

describe("parseLegionProjectFile", () => {
  it("reads project defaults from JSONC with a URL in a block comment", () => {
    expect(
      parseLegionProjectFile(`{
        /* See https://legion.codes/schema/legioncode.json */
        "defaultThreadEnvMode": "worktree"
      }`),
    ).toEqual({ defaultThreadEnvMode: "worktree" });
  });
});

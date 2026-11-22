import { assert, it } from "@effect/vitest";

import {
  isBinaryAuditTarget,
  isExcludedAuditPath,
  isOwnedAuditPath,
  scanRebrandAuditContent,
} from "./verify-rebrand-audit.ts";

const legacy = {
  codeLower: ["t3", "code"].join(""),
  codeSpaced: ["T3", "Code"].join(" "),
  codeDashed: ["T3", "Code"].join("-"),
  codeUnderscored: ["t3", "code"].join("_"),
  scope: ["@t3", "tools/shared"].join(""),
  domain: ["t3", "codes"].join("."),
  connect: ["T3", "Connect"].join(" "),
  org: ["ping", "dotgg"].join(""),
  env: ["T3", "CODE_HOME"].join(""),
  gitRef: ["refs", "t3", "checkpoints"].join("/"),
  config: ["t3", "json"].join("."),
  bundle: ["com", ["t3", "tools"].join(""), "app"].join("."),
} as const;

it("excludes third-party, vendor, lockfile, and self paths", () => {
  assert.isTrue(isExcludedAuditPath(".repos/effect/README.md"));
  assert.isTrue(isExcludedAuditPath("third-party-licenses/LICENSE"));
  assert.isTrue(isExcludedAuditPath("patches/foo.patch"));
  assert.isTrue(isExcludedAuditPath("apps/mobile/modules/legion-terminal/Vendor/Ghostty.h"));
  assert.isTrue(isExcludedAuditPath("infra/task-cache/vendor/cache.js"));
  assert.isTrue(isExcludedAuditPath("native/libghostty-vt/build.zig"));
  assert.isTrue(isExcludedAuditPath("pnpm-lock.yaml"));
  assert.isTrue(isExcludedAuditPath("scripts/verify-rebrand-audit.ts"));
  assert.isFalse(isExcludedAuditPath("apps/server/src/index.ts"));
});

it("flags every unallowlisted legacy brand pattern with exact file and line numbers", () => {
  const sample = [
    "const ok = 'LegionCode';",
    `const a = '${legacy.codeLower}';`,
    `const b = '${legacy.codeSpaced}';`,
    `const c = '${legacy.codeDashed}';`,
    `const d = '${legacy.codeUnderscored}';`,
    `const e = '${legacy.scope}';`,
    `const f = '${legacy.domain}';`,
    `const g = '${legacy.connect}';`,
    `const h = '${legacy.org}';`,
    `const i = '${legacy.env}';`,
    `const j = '${legacy.gitRef}';`,
    `const k = '${legacy.config}';`,
    `const l = '${legacy.bundle}';`,
  ].join("\n");

  const findings = scanRebrandAuditContent("apps/server/src/sample.ts", sample);
  assert.deepStrictEqual(
    findings.map((finding) => finding.line),
    [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
  );
  assert.isTrue(findings.every((finding) => finding.file === "apps/server/src/sample.ts"));
});

it("allows pinned upstream fork URLs unless another legacy token shares the line", () => {
  const ghosttyUrl = [
    "https://github.com/Yash-Singh1/ghostty/tree/",
    legacy.codeLower,
    "/custom-io-ordered-feed",
  ].join("");
  assert.deepStrictEqual(
    scanRebrandAuditContent("third-party-licenses.config.json", ghosttyUrl),
    [],
  );

  const screensForkUrl = `https://github.com/${legacy.org}/react-native-screens/tree/t3-v5.0.0-t3.58`;
  assert.deepStrictEqual(
    scanRebrandAuditContent("docs/operations/mobile-screens-fork.md", screensForkUrl),
    [],
  );

  const mixed = `${ghosttyUrl} ${legacy.scope}`;
  const findings = scanRebrandAuditContent("third-party-licenses.config.json", mixed);
  assert.equal(findings.length, 1);
  assert.equal(findings[0]?.rule, ["@t3", "tools"].join(""));
});

it("recognizes owned paths and binary payloads", () => {
  assert.isTrue(isOwnedAuditPath("scripts/dev-runner.ts"));
  assert.isTrue(isOwnedAuditPath("apps/web/public/favicon.ico"));
  assert.isTrue(isOwnedAuditPath("apps/mobile/assets/android-icon.png"));
  assert.isFalse(isOwnedAuditPath("apps/server/src/main.ts"));

  assert.isTrue(isBinaryAuditTarget("assets/prod/icon.png", new Uint8Array([1, 2, 3])));
  assert.isTrue(isBinaryAuditTarget("scripts/fixture.bin", new Uint8Array([1, 0, 2])));
  assert.isFalse(isBinaryAuditTarget("scripts/test.ts", new TextEncoder().encode("legioncode")));
});

import { assert, it } from "@effect/vitest";

import {
  repositoryPathFromRemoteUrl,
  transportSafeSourceControlErrorValue,
} from "./SourceControlProvider.ts";

it("removes URL credentials, query parameters, and fragments from error transport values", () => {
  assert.strictEqual(
    transportSafeSourceControlErrorValue(
      "https://user:secret@example.test/org/repo/pull/42?token=secret#discussion",
    ),
    "https://example.test/org/repo/pull/42",
  );
});

it.each([
  ["git@github.com:9thLevelSoftware/legioncode.git", "9thLevelSoftware/legioncode"],
  ["github.com:9thLevelSoftware/legioncode.git", "9thLevelSoftware/legioncode"],
  ["gitlab.com:9thLevelSoftware/platform/legioncode", "9thLevelSoftware/platform/legioncode"],
  ["https://github.com/9thLevelSoftware/legioncode.git", "9thLevelSoftware/legioncode"],
  [
    "https://github.com/9thLevelSoftware/legioncode.git?view=compact",
    "9thLevelSoftware/legioncode",
  ],
  ["https://github.com/9thLevelSoftware/legioncode.git#section", "9thLevelSoftware/legioncode"],
  ["C:/repos/legioncode", null],
])("reads the repository path of %s", (remoteUrl, expected) => {
  assert.strictEqual(repositoryPathFromRemoteUrl(remoteUrl), expected);
});

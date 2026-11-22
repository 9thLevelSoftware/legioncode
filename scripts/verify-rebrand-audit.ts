#!/usr/bin/env node

import * as NodeRuntime from "@effect/platform-node/NodeRuntime";
import * as NodeServices from "@effect/platform-node/NodeServices";
import * as Console from "effect/Console";
import * as Effect from "effect/Effect";
import * as FileSystem from "effect/FileSystem";
import * as Layer from "effect/Layer";
import * as Logger from "effect/Logger";
import * as Option from "effect/Option";
import * as Path from "effect/Path";
import * as Schema from "effect/Schema";
import { Command, Flag } from "effect/cli";
import { ChildProcess, ChildProcessSpawner } from "effect/process";

export interface RebrandAuditFinding {
  readonly file: string;
  readonly line: number;
  readonly rule: string;
  readonly snippet: string;
}

const EXCLUDED_PREFIXES = [
  ".repos/",
  "third-party-licenses/",
  "patches/",
  "infra/task-cache/vendor/",
  "native/libghostty-vt/",
] as const;

const EXCLUDED_EXACT = new Set(["pnpm-lock.yaml", "scripts/verify-rebrand-audit.ts"]);

const MOBILE_VENDOR_PATTERN = /^apps\/mobile\/modules\/[^/]+\/Vendor\//u;

const OWNED_PATH_PREFIXES = [
  "assets/",
  "oxlint-plugin-legioncode/",
  "scripts/",
  "apps/mobile/deps/",
] as const;

const OWNED_EXACT_FILES = new Set([
  "package.json",
  "pnpm-workspace.yaml",
  "knip.jsonc",
  "vite.config.ts",
  "legioncode.json",
  "third-party-licenses.config.json",
  ".env.example",
  ".gitignore",
  ".coderabbit.config.ts",
  ".task-cache.json",
  "apps/web/public/apple-touch-icon.png",
  "apps/web/public/favicon-16x16.png",
  "apps/web/public/favicon-32x32.png",
  "apps/web/public/favicon.ico",
  "apps/web/public/icon-192x192.png",
  "apps/web/public/icon-512x512.png",
]);

const OWNED_ASSET_PATTERN =
  /^(?:apps\/web\/public\/(?:favicon.*|apple-touch-icon\.png|icon-.*\.png)|apps\/mobile\/assets\/android-.*\.png)$/u;

const BINARY_EXTENSIONS = new Set([
  ".a",
  ".appimage",
  ".dmg",
  ".exe",
  ".gif",
  ".gz",
  ".icns",
  ".ico",
  ".jar",
  ".jpeg",
  ".jpg",
  ".keystore",
  ".node",
  ".o",
  ".otf",
  ".png",
  ".sqlite",
  ".sqlite-shm",
  ".sqlite-wal",
  ".tgz",
  ".ttf",
  ".wasm",
  ".webp",
  ".woff",
  ".woff2",
  ".zip",
]);

interface AuditRule {
  readonly name: string;
  readonly pattern: RegExp;
}

// Keep specific prefixed rules before the broad `t3code` rule so diagnostics stay informative.
const AUDIT_RULES: ReadonlyArray<AuditRule> = [
  { name: "T3CODE_", pattern: /T3CODE_/i },
  { name: "@t3tools", pattern: /@t3tools/i },
  { name: "com.t3tools", pattern: /com\.t3tools/i },
  { name: "t3.codes", pattern: /t3\.codes/i },
  { name: "T3 Code", pattern: /t3 code/i },
  { name: "T3-Code", pattern: /t3-code/i },
  { name: "t3_code", pattern: /t3_code/i },
  { name: "T3 Connect", pattern: /t3\s+connect/i },
  { name: "pingdotgg", pattern: /pingdotgg/i },
  { name: "refs/t3", pattern: /refs\/t3/i },
  { name: "t3.json", pattern: /t3\.json/i },
  { name: "t3code", pattern: /t3code/i },
];

// External upstream references that are legitimately pinned to a third-party branch/fork.
const ALLOWLISTED_LINE_SUBSTRINGS = [
  "https://github.com/Yash-Singh1/ghostty/tree/t3code/custom-io-ordered-feed",
  "https://github.com/pingdotgg/react-native-screens",
  "pingdotgg/react-native-screens",
] as const;

export class RebrandAuditCommandError extends Schema.TaggedError<RebrandAuditCommandError>()(
  "RebrandAuditCommandError",
  {
    detail: Schema.String,
    cause: Schema.optional(Schema.Defect()),
  },
) {
  override get message(): string {
    return this.detail;
  }
}

export class RebrandAuditFailedError extends Schema.TaggedError<RebrandAuditFailedError>()(
  "RebrandAuditFailedError",
  {
    count: Schema.Number,
  },
) {
  override get message(): string {
    return `Rebrand audit found ${String(this.count)} unallowlisted legacy reference(s).`;
  }
}

export function normalizeRepoPath(filePath: string): string {
  return filePath.replaceAll("\\", "/").replace(/^\.\//, "");
}

export function isExcludedAuditPath(filePath: string): boolean {
  const normalized = normalizeRepoPath(filePath);
  if (EXCLUDED_EXACT.has(normalized)) {
    return true;
  }
  if (MOBILE_VENDOR_PATTERN.test(normalized)) {
    return true;
  }
  return EXCLUDED_PREFIXES.some((prefix) => normalized.startsWith(prefix));
}

export function isOwnedAuditPath(filePath: string): boolean {
  const normalized = normalizeRepoPath(filePath);
  return (
    OWNED_EXACT_FILES.has(normalized) ||
    OWNED_ASSET_PATTERN.test(normalized) ||
    OWNED_PATH_PREFIXES.some((prefix) => normalized.startsWith(prefix))
  );
}

export function matchesRequestedPaths(
  filePath: string,
  requestedPaths: ReadonlyArray<string>,
): boolean {
  if (requestedPaths.length === 0) {
    return true;
  }
  const normalized = normalizeRepoPath(filePath);
  return requestedPaths.some((candidate) => {
    const clean = normalizeRepoPath(candidate).replace(/\/+$/, "");
    return clean.length > 0 && (normalized === clean || normalized.startsWith(`${clean}/`));
  });
}

function stripAllowlistedSubstrings(line: string): string {
  let sanitized = line;
  for (const allowed of ALLOWLISTED_LINE_SUBSTRINGS) {
    sanitized = sanitized.replaceAll(allowed, "");
  }
  return sanitized;
}

function matchAuditRule(text: string): AuditRule | undefined {
  return AUDIT_RULES.find((rule) => rule.pattern.test(text));
}

export function isBinaryAuditTarget(filePath: string, bytes: Uint8Array): boolean {
  const lastDot = filePath.lastIndexOf(".");
  if (lastDot !== -1 && BINARY_EXTENSIONS.has(filePath.slice(lastDot).toLowerCase())) {
    return true;
  }
  const sample = bytes.subarray(0, 8192);
  return sample.includes(0);
}

export function scanRebrandAuditContent(
  filePath: string,
  contents: string,
): ReadonlyArray<RebrandAuditFinding> {
  const normalized = normalizeRepoPath(filePath);
  if (isExcludedAuditPath(normalized)) {
    return [];
  }
  const findings: RebrandAuditFinding[] = [];
  const pathRule = matchAuditRule(normalized);
  if (pathRule) {
    findings.push({
      file: normalized,
      line: 1,
      rule: `${pathRule.name} (path)`,
      snippet: normalized,
    });
  }
  const lines = contents.split(/\r?\n/u);
  for (let index = 0; index < lines.length; index++) {
    const rawLine = lines[index] ?? "";
    const sanitized = stripAllowlistedSubstrings(rawLine);
    const rule = matchAuditRule(sanitized);
    if (rule) {
      findings.push({
        file: normalized,
        line: index + 1,
        rule: rule.name,
        snippet: rawLine.trim().slice(0, 160),
      });
    }
  }
  return findings;
}

const listTrackedRepoFiles = Effect.fn("verifyRebrandAudit.listTrackedRepoFiles")(function* (
  repoRoot: string,
) {
  const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
  const raw = yield* spawner
    .string(ChildProcess.make("git", ["ls-files", "-z"], { cwd: repoRoot }))
    .pipe(
      Effect.mapError(
        (cause) =>
          new RebrandAuditCommandError({
            detail: "Failed to run `git ls-files -z` for rebrand audit.",
            cause,
          }),
      ),
    );
  return raw
    .split("\0")
    .map((entry) => normalizeRepoPath(entry))
    .filter((entry) => entry.length > 0);
});

export const runRebrandAudit = Effect.fn("verifyRebrandAudit.run")(function* (options: {
  readonly repoRoot?: string;
  readonly ownedOnly?: boolean;
  readonly paths?: ReadonlyArray<string>;
}) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const repoRoot = options.repoRoot ?? (yield* path.fromFileUrl(new URL("..", import.meta.url)));
  const requestedPaths = options.paths ?? [];

  const trackedFiles = yield* listTrackedRepoFiles(repoRoot);
  const candidates = trackedFiles.filter(
    (relativePath) =>
      !isExcludedAuditPath(relativePath) &&
      (!options.ownedOnly || isOwnedAuditPath(relativePath)) &&
      matchesRequestedPaths(relativePath, requestedPaths),
  );

  const findings: RebrandAuditFinding[] = [];
  for (const relativePath of candidates) {
    const absolutePath = path.join(repoRoot, relativePath);
    if (!(yield* fs.exists(absolutePath))) {
      continue;
    }
    const bytes = yield* fs.readFile(absolutePath).pipe(
      Effect.mapError(
        (cause) =>
          new RebrandAuditCommandError({
            detail: `Failed to read ${relativePath}.`,
            cause,
          }),
      ),
    );
    if (isBinaryAuditTarget(relativePath, bytes)) {
      const pathRule = matchAuditRule(relativePath);
      if (pathRule) {
        findings.push({
          file: relativePath,
          line: 1,
          rule: `${pathRule.name} (path)`,
          snippet: relativePath,
        });
      }
      continue;
    }
    const contents = new TextDecoder().decode(bytes);
    findings.push(...scanRebrandAuditContent(relativePath, contents));
  }

  return { scannedFiles: candidates.length, findings } as const;
});

const command = Command.make(
  "verify-rebrand-audit",
  {
    owned: Flag.Boolean("owned").pipe(
      Flag.withDefault(false),
      Flag.withDescription("Audit only assets, scripts, lint plugin, and owned root configs."),
    ),
    paths: Flag.String("paths").pipe(
      Flag.optional,
      Flag.withDescription("Comma-separated path prefixes or exact repo files to audit."),
    ),
  },
  (input) =>
    Effect.gen(function* () {
      const requestedPaths = Option.isSome(input.paths)
        ? input.paths.value
            .split(",")
            .map((entry) => entry.trim())
            .filter((entry) => entry.length > 0)
        : [];
      const result = yield* runRebrandAudit({
        ownedOnly: input.owned,
        paths: requestedPaths,
      });
      if (result.findings.length > 0) {
        for (const finding of result.findings) {
          yield* Console.error(
            `${finding.file}:${String(finding.line)} [${finding.rule}] ${finding.snippet}`,
          );
        }
        return yield* new RebrandAuditFailedError({ count: result.findings.length });
      }
      yield* Console.log(
        `Rebrand audit passed across ${String(result.scannedFiles)} tracked file(s).`,
      );
    }),
).pipe(Command.withDescription("Verify zero unallowlisted legacy T3 references remain."));

if (import.meta.main) {
  Command.run(command, { version: "0.0.0" }).pipe(
    Effect.provide(Layer.mergeAll(Logger.layer([Logger.consolePretty()]), NodeServices.layer)),
    NodeRuntime.runMain,
  );
}

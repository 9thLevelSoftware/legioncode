import * as Schema from "effect/Schema";

const WORKFLOW_BOARD_ARTIFACT_KEY_MAX_LENGTH = 256;

export const isWithinArtifactKeyMaxScalars = Schema.makeFilter(
  (value: string) => [...value.trim()].length <= WORKFLOW_BOARD_ARTIFACT_KEY_MAX_LENGTH,
  {
    expected: `a string with at most ${WORKFLOW_BOARD_ARTIFACT_KEY_MAX_LENGTH} Unicode scalar values`,
  },
);

export const isSafeRelativeArtifactKey = Schema.makeFilter(
  (value: string) => {
    const trimmed = value.trim();
    if (
      trimmed.length === 0 ||
      trimmed.includes("\0") ||
      trimmed.includes("\\") ||
      trimmed.includes(":")
    ) {
      return false;
    }
    if (trimmed.startsWith("/")) {
      return false;
    }
    const segments = trimmed.split("/");
    return segments.every((seg) => seg.length > 0 && seg !== "." && seg !== "..");
  },
  { expected: "a safe relative artifact key without traversal or root/drive prefixes" },
);

const CREDENTIAL_EXTENSIONS = [".pem", ".key", ".p12", ".pfx", ".crt", ".der"] as const;
const CREDENTIAL_BASENAMES = new Set(["id_rsa", "id_ed25519", "id_ecdsa", ".npmrc", ".netrc"]);

function isCredentialFilePath(raw: string): boolean {
  const normalized = raw.trim().replaceAll("\\", "/").toLowerCase();
  if (normalized.length === 0) {
    return false;
  }
  const segments = normalized.split("/");
  if (segments.includes(".ssh")) {
    return true;
  }
  const basename = segments[segments.length - 1] ?? "";
  if (basename === ".env" || basename.startsWith(".env.")) {
    return true;
  }
  if (CREDENTIAL_BASENAMES.has(basename)) {
    return true;
  }
  return CREDENTIAL_EXTENSIONS.some((ext) => basename.endsWith(ext));
}

function isGitMetadataSegment(seg: string): boolean {
  const head = seg.split(":")[0] ?? "";
  const cleaned = head.replace(/[. ]+$/, "").toUpperCase();
  return cleaned === ".GIT" || cleaned === "GIT~1";
}

export const isNotGitMetadataPath = Schema.makeFilter(
  (value: string) => !value.trim().split("/").some(isGitMetadataSegment),
  { expected: "a workspace relative path outside .git metadata" },
);

export const isNotCredentialFilePath = Schema.makeFilter(
  (value: string) => !isCredentialFilePath(value),
  { expected: "a non-credential workspace relative path" },
);

const POSIX_SHELL_BINARIES = new Set([
  "sh",
  "bash",
  "zsh",
  "dash",
  "fish",
  "ksh",
  "ash",
  "csh",
  "tcsh",
]);

const SHELL_CONTROL_SNIPPETS = ["&&", "||", "$(", "`", ";", "|", ">", "<"] as const;

function executableBasename(token: string): string {
  const parts = token.trim().split(/[/\\]/);
  const last = (parts[parts.length - 1] ?? "").toLowerCase();
  return last.endsWith(".exe") ? last.slice(0, -4) : last;
}

function isPosixInlineFlag(arg: string): boolean {
  const lower = arg.trim().toLowerCase();
  if (lower === "-c" || lower === "-lc" || lower === "-ic") {
    return true;
  }
  return lower.startsWith("-") && !lower.startsWith("--") && lower.slice(1).includes("c");
}

function isPowerShellInlineFlag(arg: string): boolean {
  const lower = arg.trim().toLowerCase();
  return (
    lower === "-c" ||
    lower === "-command" ||
    lower === "-encodedcommand" ||
    lower === "-enc" ||
    lower === "-e"
  );
}

function isEnvSplitStringFlag(arg: string): boolean {
  const trimmed = arg.trim();
  const lower = trimmed.toLowerCase();
  if (lower === "--split-string" || lower.startsWith("--split-string=")) {
    return true;
  }
  return trimmed.startsWith("-") && !trimmed.startsWith("--") && trimmed.slice(1).includes("S");
}

function isEnvTwoTokenFlag(arg: string): boolean {
  if (
    arg === "-u" ||
    arg === "--unset" ||
    arg === "-C" ||
    arg === "--chdir" ||
    arg === "-a" ||
    arg === "--argv0"
  ) {
    return true;
  }
  if (arg.startsWith("-") && !arg.startsWith("--") && arg.length > 2) {
    const rest = arg.slice(1);
    const last = rest[rest.length - 1] ?? "";
    const prefix = rest.slice(0, -1);
    return (
      (last === "u" || last === "C" || last === "a") &&
      [...prefix].every((ch) => ch === "i" || ch === "0" || ch === "v")
    );
  }
  return false;
}

function isEnvSingleTokenFlag(arg: string): boolean {
  if (
    arg === "-i" ||
    arg === "--ignore-environment" ||
    arg === "-0" ||
    arg === "--null" ||
    arg === "-v" ||
    arg === "--debug" ||
    arg.startsWith("--unset=") ||
    arg.startsWith("--chdir=") ||
    arg.startsWith("--argv0=")
  ) {
    return true;
  }
  if (arg.startsWith("-") && !arg.startsWith("--") && arg.length > 1) {
    const rest = arg.slice(1);
    if ((rest.startsWith("u") || rest.startsWith("C") || rest.startsWith("a")) && rest.length > 1) {
      return true;
    }
    return [...rest].every((ch) => ch === "i" || ch === "0" || ch === "v");
  }
  return false;
}

function isSafeArgvCommand(argv: ReadonlyArray<string>): boolean {
  if (argv.length === 0) {
    return false;
  }
  for (const token of argv) {
    if (token.trim().length === 0) {
      return false;
    }
    if (token.includes("\0") || token.includes("\r") || token.includes("\n")) {
      return false;
    }
    if (SHELL_CONTROL_SNIPPETS.some((snippet) => token.includes(snippet))) {
      return false;
    }
  }

  let exeIdx = 0;
  while (exeIdx < argv.length && executableBasename(argv[exeIdx] ?? "") === "env") {
    exeIdx++;
    while (exeIdx < argv.length) {
      const arg = (argv[exeIdx] ?? "").trim();
      if (isEnvSplitStringFlag(arg)) {
        return false;
      }
      if (arg === "--") {
        exeIdx++;
        break;
      }
      if (isEnvTwoTokenFlag(arg)) {
        if (exeIdx + 1 >= argv.length) {
          return false;
        }
        exeIdx += 2;
        continue;
      }
      if (isEnvSingleTokenFlag(arg) || (!arg.startsWith("-") && arg.includes("="))) {
        exeIdx++;
        continue;
      }
      if (arg.startsWith("-")) {
        return false;
      }
      break;
    }
  }
  if (exeIdx >= argv.length) {
    return false;
  }

  const exe = executableBasename(argv[exeIdx] ?? "");
  const rest = argv.slice(exeIdx + 1);
  if (POSIX_SHELL_BINARIES.has(exe) && rest.some(isPosixInlineFlag)) {
    return false;
  }
  if (exe === "cmd" && rest.some((arg) => ["/c", "/k"].includes(arg.trim().toLowerCase()))) {
    return false;
  }
  if ((exe === "powershell" || exe === "pwsh") && rest.some(isPowerShellInlineFlag)) {
    return false;
  }

  return true;
}

export const isSafeArgvCommandFilter = Schema.makeFilter(
  (argv: ReadonlyArray<string>) => isSafeArgvCommand(argv),
  { expected: "an argv-only command without shell wrappers or shell control operators" },
);

export function maxLengthFilter<T>(max: number, label: string) {
  return Schema.makeFilter((items: ReadonlyArray<T>) => items.length <= max, {
    expected: `at most ${max} ${label}`,
  });
}

export interface LensLike {
  readonly lens: string;
}

export function areUniqueLenses(lenses: ReadonlyArray<LensLike>): boolean {
  const seen = new Set<string>();
  for (const entry of lenses) {
    if (seen.has(entry.lens)) {
      return false;
    }
    seen.add(entry.lens);
  }
  return true;
}

export interface AlternativeLike {
  readonly id: string;
  readonly title: string;
  readonly summary: string;
  readonly tradeoffs: string;
}

export function areDistinctPlanAlternatives(alts: ReadonlyArray<AlternativeLike>): boolean {
  if (alts.length !== 2) {
    return false;
  }
  const [first, second] = alts as readonly [AlternativeLike, AlternativeLike];
  if (first.id.trim() === second.id.trim()) {
    return false;
  }
  return !(
    first.title.trim() === second.title.trim() &&
    first.summary.trim() === second.summary.trim() &&
    first.tradeoffs.trim() === second.tradeoffs.trim()
  );
}

export interface TaskContractLike {
  readonly taskId: string;
  readonly dependsOn?: ReadonlyArray<string> | undefined;
  readonly blockedBy?: ReadonlyArray<string> | undefined;
  readonly blocks?: ReadonlyArray<string> | undefined;
  readonly filesAllowed: ReadonlyArray<string>;
  readonly filesForbidden?: ReadonlyArray<string> | undefined;
}

export function areDisjointTaskFiles(tasks: ReadonlyArray<TaskContractLike>): boolean {
  const claimedAllowed = new Set<string>();
  for (const task of tasks) {
    const forbidden = new Set((task.filesForbidden ?? []).map((p) => p.trim()));
    for (const allowed of task.filesAllowed) {
      const normalized = allowed.trim();
      if (forbidden.has(normalized) || claimedAllowed.has(normalized)) {
        return false;
      }
      claimedAllowed.add(normalized);
    }
  }
  return true;
}

export function isAcyclicTaskDag(tasks: ReadonlyArray<TaskContractLike>): boolean {
  const ids = new Set<string>();
  for (const task of tasks) {
    const id = task.taskId.trim();
    if (id.length === 0 || ids.has(id)) {
      return false;
    }
    ids.add(id);
  }

  const dependents = new Map<string, Set<string>>();
  const inDegree = new Map<string, number>();
  for (const id of ids) {
    dependents.set(id, new Set());
    inDegree.set(id, 0);
  }

  for (const task of tasks) {
    const id = task.taskId.trim();
    const deps = [
      ...(task.dependsOn ?? []).map((d) => d.trim()),
      ...(task.blockedBy ?? []).map((d) => d.trim()),
    ];
    for (const dep of deps) {
      if (!ids.has(dep) || dep === id) {
        return false;
      }
      const list = dependents.get(dep);
      if (list && !list.has(id)) {
        list.add(id);
        inDegree.set(id, (inDegree.get(id) ?? 0) + 1);
      }
    }
    for (const rawBlocked of task.blocks ?? []) {
      const blocked = rawBlocked.trim();
      if (!ids.has(blocked) || blocked === id) {
        return false;
      }
      const list = dependents.get(id);
      if (list && !list.has(blocked)) {
        list.add(blocked);
        inDegree.set(blocked, (inDegree.get(blocked) ?? 0) + 1);
      }
    }
  }

  const queue: string[] = [];
  for (const [id, deg] of inDegree.entries()) {
    if (deg === 0) {
      queue.push(id);
    }
  }

  let visited = 0;
  while (queue.length > 0) {
    const current = queue.shift()!;
    visited++;
    for (const next of dependents.get(current) ?? []) {
      const nextDeg = (inDegree.get(next) ?? 0) - 1;
      inDegree.set(next, nextDeg);
      if (nextDeg === 0) {
        queue.push(next);
      }
    }
  }

  return visited === ids.size;
}

export interface AcMappingLike {
  readonly criterionId: string;
}

export function areUniqueAcCriterionIds(mappings: ReadonlyArray<AcMappingLike>): boolean {
  const seen = new Set<string>();
  for (const mapping of mappings) {
    const id = mapping.criterionId.trim();
    if (seen.has(id)) {
      return false;
    }
    seen.add(id);
  }
  return true;
}

import * as Option from "effect/Option";

export type JoinPath = (first: string, ...segments: string[]) => string;

function normalizeConfiguredBaseDir(legioncodeHome: Option.Option<string>): Option.Option<string> {
  if (Option.isNone(legioncodeHome)) {
    return Option.none();
  }
  const trimmed = legioncodeHome.value.trim();
  return trimmed.length > 0 ? Option.some(trimmed) : Option.none();
}

export function resolveDesktopBaseDir(input: {
  readonly homeDirectory: string;
  readonly joinPath: JoinPath;
  readonly legioncodeHome: Option.Option<string>;
}): string {
  return Option.getOrElse(normalizeConfiguredBaseDir(input.legioncodeHome), () =>
    input.joinPath(input.homeDirectory, ".legioncode"),
  );
}

export function resolveDesktopStateDir(input: {
  readonly baseDir: string;
  readonly isDevelopment: boolean;
  readonly joinPath: JoinPath;
  readonly legioncodeHome: Option.Option<string>;
}): string {
  const useDevSubdir =
    input.isDevelopment && Option.isNone(normalizeConfiguredBaseDir(input.legioncodeHome));
  return input.joinPath(input.baseDir, useDevSubdir ? "dev" : "userdata");
}

import {
  LEGION_PROJECT_FILE_NAME,
  type EnvironmentId,
  type LegionProjectFile,
  type LegionProjectFileScript,
} from "@legioncode/contracts";
import { parseLegionProjectFile } from "@legioncode/shared/legionProjectFile";

import { useProjectFileQuery } from "~/components/files/projectFilesQueryState";

const NO_SCRIPTS: ReadonlyArray<LegionProjectFileScript> = [];

export interface LegionProjectFileState {
  status: "loading" | "missing" | "invalid" | "valid";
  file: LegionProjectFile | null;
  scripts: ReadonlyArray<LegionProjectFileScript>;
}

export function useLegionProjectFileState(
  environmentId: EnvironmentId,
  cwd: string | null,
): LegionProjectFileState {
  const query = useProjectFileQuery(
    environmentId,
    cwd ?? "",
    LEGION_PROJECT_FILE_NAME,
    cwd !== null,
  );
  const contents = query.data && !query.data.truncated ? query.data.contents : null;
  const isPending = query.isPending;
  if (contents === null) {
    return {
      status: isPending ? "loading" : "missing",
      file: null,
      scripts: NO_SCRIPTS,
    } as const;
  }
  const file = parseLegionProjectFile(contents);
  if (file === null) {
    return { status: "invalid", file: null, scripts: NO_SCRIPTS } as const;
  }
  return { status: "valid", file, scripts: file.scripts ?? NO_SCRIPTS } as const;
}

export function useLegionProjectFileScripts(
  environmentId: EnvironmentId,
  cwd: string | null,
): ReadonlyArray<LegionProjectFileScript> {
  return useLegionProjectFileState(environmentId, cwd).scripts;
}

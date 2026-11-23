import {
  LEGION_PROJECT_FILE_NAME,
  type EnvironmentId,
  type LegionProjectFile,
  type ProjectReadFileResult,
} from "@legioncode/contracts";
import { parseLegionProjectFile } from "@legioncode/shared/legionProjectFile";
import { executeAtomQuery } from "@legioncode/client-runtime/state/runtime";
import { AsyncResult } from "effect/reactivity";

import {
  getProjectFileQueryAtom,
  resolveProjectFileQueryData,
} from "~/components/files/projectFilesQueryState";
import { appAtomRegistry } from "~/rpc/atomRegistry";

function toLegionProjectFile(
  environmentId: EnvironmentId,
  workspaceRoot: string,
  result: ProjectReadFileResult | null,
): LegionProjectFile | null {
  const data = resolveProjectFileQueryData(
    environmentId,
    workspaceRoot,
    LEGION_PROJECT_FILE_NAME,
    result,
  );
  if (data === null || data.exists === false || data.truncated) return null;
  return parseLegionProjectFile(data.contents);
}

export async function readLegionProjectFile(
  environmentId: EnvironmentId,
  workspaceRoot: string,
): Promise<LegionProjectFile | null> {
  const result = await executeAtomQuery(
    appAtomRegistry,
    getProjectFileQueryAtom(environmentId, workspaceRoot, LEGION_PROJECT_FILE_NAME),
    { reportDefect: false, reportFailure: false },
  );
  return toLegionProjectFile(
    environmentId,
    workspaceRoot,
    result._tag === "Success" ? result.value : null,
  );
}

/** The project's legioncode.json from an earlier read, or `undefined` when none has loaded yet. */
export function peekLegionProjectFile(
  environmentId: EnvironmentId,
  workspaceRoot: string,
): LegionProjectFile | null | undefined {
  const result = appAtomRegistry.get(
    getProjectFileQueryAtom(environmentId, workspaceRoot, LEGION_PROJECT_FILE_NAME),
  );
  return AsyncResult.isSuccess(result)
    ? toLegionProjectFile(environmentId, workspaceRoot, result.value)
    : undefined;
}

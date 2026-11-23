import {
  LEGION_PROJECT_FILE_NAME,
  type EnvironmentId,
  type LegionProjectFile,
} from "@legioncode/contracts";
import { parseLegionProjectFile } from "@legioncode/shared/legionProjectFile";
import { executeAtomQuery } from "@legioncode/client-runtime/state/runtime";

import {
  getProjectFileQueryAtom,
  resolveProjectFileQueryData,
} from "~/components/files/projectFilesQueryState";
import { appAtomRegistry } from "~/rpc/atomRegistry";

export async function readLegionProjectFile(
  environmentId: EnvironmentId,
  workspaceRoot: string,
): Promise<LegionProjectFile | null> {
  const result = await executeAtomQuery(
    appAtomRegistry,
    getProjectFileQueryAtom(environmentId, workspaceRoot, LEGION_PROJECT_FILE_NAME),
    { reportDefect: false, reportFailure: false },
  );
  const data = resolveProjectFileQueryData(
    environmentId,
    workspaceRoot,
    LEGION_PROJECT_FILE_NAME,
    result._tag === "Success" ? result.value : null,
  );
  if (data === null || data.exists === false || data.truncated) return null;
  return parseLegionProjectFile(data.contents);
}

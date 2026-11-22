import { resolveFilesystemReadAccess } from "@legioncode/client-runtime/state/filesystem";
import { isScratchProject } from "@legioncode/client-runtime/state/projects";
import type { EnvironmentProject } from "@legioncode/client-runtime/state/shell";
import type { ProjectReadFileResult } from "@legioncode/contracts";
import { DEFAULT_SERVER_SETTINGS, LEGION_PROJECT_FILE_NAME } from "@legioncode/contracts";
import {
  resolveNewThreadEnvMode,
  resolveProjectSettings,
} from "@legioncode/shared/projectSettings";
import { parseLegionProjectFile } from "@legioncode/shared/legionProjectFile";

import { m } from "../../i18n/messages";
import { useEnvironmentServerConfig } from "../../state/entities";
import { useEnvironmentPresentation } from "../../state/presentation";
import { useProjectHasThreads } from "../../state/project-has-threads";
import { projectEnvironment } from "../../state/projects";
import { useEnvironmentQuery } from "../../state/query";
import { environmentSession } from "../../state/session";

/**
 * A project's new-task defaults: server settings merged with its legioncode.json. The
 * new-task flow and the home pending-drafts list both read the default
 * workspace mode here, so a draft that never picked one shows what it sends.
 * Every read is a shared per-environment or per-project atom.
 */
export function useNewTaskProjectDefaults(project: EnvironmentProject | null) {
  const serverConfig = useEnvironmentServerConfig(project?.environmentId ?? null);
  const isScratch =
    project !== null && isScratchProject(project, serverConfig?.scratchWorkspaceRoot);
  const hasWorkspace = project !== null && project.workspaceRoot !== "";
  const fileAccessSession = useEnvironmentQuery(
    hasWorkspace ? environmentSession.sessionStateAtom(project.environmentId) : null,
  );
  const fileEnvironment = useEnvironmentPresentation(project?.environmentId ?? null);
  const fileAccess = resolveFilesystemReadAccess(
    {
      isCatalogReady: fileEnvironment.isReady,
      connection: fileEnvironment.presentation?.connection ?? null,
      session: fileAccessSession.data,
      sessionError: fileAccessSession.error,
    },
    m,
  );
  const fileAccessPending = hasWorkspace && fileAccess.isPending;
  const legionProjectFileQuery = useEnvironmentQuery(
    fileAccess.canReadFiles && hasWorkspace
      ? projectEnvironment.readFile({
          environmentId: project.environmentId,
          input: {
            cwd: project.workspaceRoot,
            relativePath: LEGION_PROJECT_FILE_NAME,
            allowMissing: true,
          },
        })
      : null,
  );
  const legionProjectFileData = legionProjectFileQuery.data as ProjectReadFileResult | null;
  const legionProjectFile =
    legionProjectFileData === null ||
    legionProjectFileData.exists === false ||
    legionProjectFileData.truncated
      ? null
      : parseLegionProjectFile(legionProjectFileData.contents);
  const projectSettings = resolveProjectSettings(
    serverConfig?.settings ?? DEFAULT_SERVER_SETTINGS,
    project?.id ?? null,
    project,
    legionProjectFile,
  );
  const projectHasThreads = useProjectHasThreads(project);
  const defaultWorkspaceMode = isScratch
    ? "local"
    : resolveNewThreadEnvMode({
        projectSettings,
        workspaceRoot: project?.workspaceRoot ?? null,
        newProjectsRoot: serverConfig?.newProjectsRoot,
        projectHasThreads,
      });
  // While legioncode.json loads, the environment default is provisional; legioncode.json may replace it.
  const defaultWorkspaceModeSettled =
    projectSettings.sources.defaultThreadEnvMode !== "environment" ||
    (!legionProjectFileQuery.isPending && !fileAccessPending);
  return {
    serverConfig,
    isScratch,
    projectSettings,
    defaultWorkspaceMode,
    defaultWorkspaceModeSettled,
  };
}

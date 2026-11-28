import {
  ProviderInstanceId,
  type EnvironmentId,
  type ModelSelection,
  type OrchestrationV2ThreadLaunchWorkspaceStrategy,
  type ProjectId,
  type ProviderInteractionMode,
  type RuntimeMode,
  type ServerProvider,
  type ServerSettings,
} from "@legioncode/contracts";
import { T, useGT, useMessages } from "gt-react";
import { useId } from "react";

import { getDefaultProviderInstanceModel } from "../../providerInstances";
import { useManagedPolicy } from "../../state/managedPolicy";
import { resolveCompatibleRuntimeMode, resolveRuntimeModeOptions } from "../chat/runtimeModeConfig";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import {
  buildWorkspaceStrategy,
  isWorkflowBoardWorkspaceStrategyValid,
  resolveBoardProviderEntries,
  type BoardWorkspaceStrategyMode,
} from "./workflowBoard.logic";

export function WorkflowBoardLaunchControls(props: {
  readonly environmentId: EnvironmentId;
  readonly projectId: ProjectId;
  readonly providers: ReadonlyArray<ServerProvider>;
  readonly settings: ServerSettings;
  readonly existingWorktreePaths: ReadonlyArray<string>;
  readonly modelSelection: ModelSelection | null;
  readonly onModelSelectionChange: (next: ModelSelection) => void;
  readonly runtimeMode: RuntimeMode;
  readonly onRuntimeModeChange: (next: RuntimeMode) => void;
  readonly interactionMode: ProviderInteractionMode;
  readonly onInteractionModeChange: (next: ProviderInteractionMode) => void;
  readonly workspaceStrategy: OrchestrationV2ThreadLaunchWorkspaceStrategy;
  readonly onWorkspaceStrategyChange: (next: OrchestrationV2ThreadLaunchWorkspaceStrategy) => void;
  readonly disabled?: boolean;
}) {
  const gt = useGT();
  const messages = useMessages();
  const managedPolicy = useManagedPolicy(props.environmentId);
  const baseRefHintId = useId();
  const worktreePathHintId = useId();

  const entries = resolveBoardProviderEntries(
    props.providers,
    props.settings,
    props.projectId,
  ).filter((entry) => entry.enabled && entry.isAvailable);

  const activeEntry =
    entries.find((entry) => entry.instanceId === props.modelSelection?.instanceId) ??
    entries[0] ??
    null;

  const activeModels = activeEntry?.models ?? [];
  const activeModelSlug =
    props.modelSelection !== null &&
    activeEntry !== null &&
    props.modelSelection.instanceId === activeEntry.instanceId
      ? props.modelSelection.model
      : (activeModels[0]?.slug ?? "");

  const supportedRuntimeModes = activeEntry?.snapshot.supportedRuntimeModes;
  const { mode: resolvedRuntimeMode, modes } = resolveCompatibleRuntimeMode(
    props.runtimeMode,
    supportedRuntimeModes,
  );
  const runtimeOptions = resolveRuntimeModeOptions(modes, managedPolicy, messages);

  const strategyMode: BoardWorkspaceStrategyMode = props.workspaceStrategy.type;
  const strategyBaseRef =
    props.workspaceStrategy.type === "worktree" ? props.workspaceStrategy.baseRef : "HEAD";
  const strategyWorktreePath =
    props.workspaceStrategy.type === "existing_worktree"
      ? props.workspaceStrategy.worktreePath
      : (props.existingWorktreePaths[0] ?? "");
  const strategyBranch = props.workspaceStrategy.branch ?? "";
  const worktreeBaseRefMissing =
    props.workspaceStrategy.type === "worktree" &&
    !isWorkflowBoardWorkspaceStrategyValid(props.workspaceStrategy);
  const existingWorktreePathMissing =
    props.workspaceStrategy.type === "existing_worktree" &&
    !isWorkflowBoardWorkspaceStrategyValid(props.workspaceStrategy);

  const strategyLabel = (mode: BoardWorkspaceStrategyMode): string => {
    switch (mode) {
      case "root":
        return gt("Project root");
      case "worktree":
        return gt("New worktree");
      case "existing_worktree":
        return gt("Existing worktree");
    }
  };

  return (
    <div className="grid gap-3 sm:grid-cols-2">
      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">
          <T>Provider</T>
        </span>
        <Select
          value={activeEntry?.instanceId ?? null}
          disabled={props.disabled || entries.length === 0}
          onValueChange={(value) => {
            if (typeof value !== "string") return;
            const instanceId = ProviderInstanceId.make(value);
            const defaultModel =
              getDefaultProviderInstanceModel(props.providers, instanceId) ??
              entries.find((candidate) => candidate.instanceId === instanceId)?.models[0]?.slug ??
              "";
            props.onModelSelectionChange({ instanceId, model: defaultModel });
          }}
        >
          <SelectTrigger size="sm" aria-label={gt("Provider")}>
            <SelectValue>{activeEntry?.displayName ?? gt("No provider available")}</SelectValue>
          </SelectTrigger>
          <SelectPopup>
            {entries.map((entry) => (
              <SelectItem key={entry.instanceId} value={entry.instanceId}>
                {entry.displayName}
              </SelectItem>
            ))}
          </SelectPopup>
        </Select>
      </div>

      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">
          <T>Model</T>
        </span>
        <Select
          value={activeModelSlug || null}
          disabled={props.disabled || activeEntry === null || activeModels.length === 0}
          onValueChange={(value) => {
            if (typeof value !== "string" || activeEntry === null) return;
            props.onModelSelectionChange({
              instanceId: activeEntry.instanceId,
              model: value,
            });
          }}
        >
          <SelectTrigger size="sm" aria-label={gt("Model")}>
            <SelectValue>
              {activeModels.find((candidate) => candidate.slug === activeModelSlug)?.name ??
                (activeModelSlug || gt("Select model"))}
            </SelectValue>
          </SelectTrigger>
          <SelectPopup>
            {activeModels.map((model) => (
              <SelectItem key={model.slug} value={model.slug}>
                {model.name}
              </SelectItem>
            ))}
          </SelectPopup>
        </Select>
      </div>

      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">
          <T>Permissions</T>
        </span>
        <Select
          value={resolvedRuntimeMode}
          disabled={props.disabled}
          onValueChange={(value) => {
            if (
              value === "approval-required" ||
              value === "auto-accept-edits" ||
              value === "auto" ||
              value === "full-access"
            ) {
              props.onRuntimeModeChange(value);
            }
          }}
        >
          <SelectTrigger size="sm" aria-label={gt("Permissions")}>
            <SelectValue>
              {runtimeOptions.find((opt) => opt.mode === resolvedRuntimeMode)?.label ??
                resolvedRuntimeMode}
            </SelectValue>
          </SelectTrigger>
          <SelectPopup>
            {runtimeOptions.map((option) => (
              <SelectItem
                key={option.mode}
                value={option.mode}
                disabled={option.disabledReason !== undefined}
              >
                {option.label}
              </SelectItem>
            ))}
          </SelectPopup>
        </Select>
      </div>

      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">
          <T>Interaction mode</T>
        </span>
        <Select
          value={props.interactionMode}
          disabled={props.disabled}
          onValueChange={(value) => {
            if (value === "default" || value === "plan") {
              props.onInteractionModeChange(value);
            }
          }}
        >
          <SelectTrigger size="sm" aria-label={gt("Interaction mode")}>
            <SelectValue>
              {props.interactionMode === "plan" ? gt("Plan") : gt("Default")}
            </SelectValue>
          </SelectTrigger>
          <SelectPopup>
            <SelectItem value="default">{gt("Default")}</SelectItem>
            <SelectItem value="plan">{gt("Plan")}</SelectItem>
          </SelectPopup>
        </Select>
      </div>

      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">
          <T>Workspace</T>
        </span>
        <Select
          value={strategyMode}
          disabled={props.disabled}
          onValueChange={(value) => {
            if (value !== "root" && value !== "worktree" && value !== "existing_worktree") return;
            props.onWorkspaceStrategyChange(
              buildWorkspaceStrategy({
                mode: value,
                baseRef: strategyBaseRef,
                worktreePath: strategyWorktreePath,
                branch: strategyBranch,
              }),
            );
          }}
        >
          <SelectTrigger size="sm" aria-label={gt("Workspace")}>
            <SelectValue>{strategyLabel(strategyMode)}</SelectValue>
          </SelectTrigger>
          <SelectPopup>
            <SelectItem value="root">{gt("Project root")}</SelectItem>
            <SelectItem value="worktree">{gt("New worktree")}</SelectItem>
            <SelectItem value="existing_worktree">{gt("Existing worktree")}</SelectItem>
          </SelectPopup>
        </Select>
      </div>

      {strategyMode === "worktree" ? (
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-medium text-muted-foreground">
            <T>Base ref</T>
          </span>
          <Input
            size="sm"
            required
            aria-label={gt("Base ref")}
            aria-invalid={worktreeBaseRefMissing || undefined}
            aria-describedby={worktreeBaseRefMissing ? baseRefHintId : undefined}
            placeholder={gt("HEAD")}
            value={strategyBaseRef}
            disabled={props.disabled}
            onValueChange={(value) =>
              props.onWorkspaceStrategyChange(
                buildWorkspaceStrategy({
                  mode: "worktree",
                  baseRef: value,
                  branch: strategyBranch,
                }),
              )
            }
          />
          {worktreeBaseRefMissing ? (
            <p id={baseRefHintId} role="alert" className="text-xs text-destructive-foreground">
              <T>Enter a base ref to continue.</T>
            </p>
          ) : null}
        </div>
      ) : null}

      {strategyMode === "existing_worktree" ? (
        <div className="flex flex-col gap-1.5 sm:col-span-2">
          <span className="text-xs font-medium text-muted-foreground">
            <T>Worktree path</T>
          </span>
          <Input
            size="sm"
            font="mono"
            required
            aria-label={gt("Worktree path")}
            aria-invalid={existingWorktreePathMissing || undefined}
            aria-describedby={existingWorktreePathMissing ? worktreePathHintId : undefined}
            placeholder={gt("/path/to/worktree")}
            value={strategyWorktreePath}
            disabled={props.disabled}
            onValueChange={(value) =>
              props.onWorkspaceStrategyChange(
                buildWorkspaceStrategy({
                  mode: "existing_worktree",
                  worktreePath: value,
                  branch: strategyBranch,
                }),
              )
            }
          />
          {existingWorktreePathMissing ? (
            <p id={worktreePathHintId} role="alert" className="text-xs text-destructive-foreground">
              <T>Enter an existing worktree path to continue.</T>
            </p>
          ) : null}
          {props.existingWorktreePaths.length > 0 ? (
            <Select
              value={
                props.existingWorktreePaths.includes(strategyWorktreePath)
                  ? strategyWorktreePath
                  : null
              }
              disabled={props.disabled}
              onValueChange={(value) => {
                if (typeof value !== "string") return;
                props.onWorkspaceStrategyChange(
                  buildWorkspaceStrategy({
                    mode: "existing_worktree",
                    worktreePath: value,
                    branch: strategyBranch,
                  }),
                );
              }}
            >
              <SelectTrigger size="sm" aria-label={gt("Known worktree")}>
                <SelectValue>{gt("Choose known worktree")}</SelectValue>
              </SelectTrigger>
              <SelectPopup>
                {props.existingWorktreePaths.map((path) => (
                  <SelectItem key={path} value={path}>
                    {path}
                  </SelectItem>
                ))}
              </SelectPopup>
            </Select>
          ) : null}
        </div>
      ) : null}

      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">
          <T>Branch (optional)</T>
        </span>
        <Input
          size="sm"
          font="mono"
          aria-label={gt("Branch")}
          placeholder={gt("Optional branch name")}
          value={strategyBranch}
          disabled={props.disabled}
          onValueChange={(value) =>
            props.onWorkspaceStrategyChange(
              buildWorkspaceStrategy({
                mode: strategyMode,
                baseRef: strategyBaseRef,
                worktreePath: strategyWorktreePath,
                branch: value,
              }),
            )
          }
        />
      </div>
    </div>
  );
}

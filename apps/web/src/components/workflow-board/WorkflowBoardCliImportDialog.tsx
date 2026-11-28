import { useAtomValue } from "@effect/atom-react";
import {
  isValidCliImportDirInput,
  workflowBoardCardStatusLabel,
  workflowBoardCliImportHistoricalReceiptKindLabel,
  workflowBoardCliImportPolicyStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardStageLabel,
} from "@legioncode/client-runtime/workflow-board";
import {
  WorkflowBoardSafeInputPath,
  type EnvironmentId,
  type ModelSelection,
  type ProjectId,
  type ServerProvider,
  type ServerSettings,
  type WorkflowBoardPreviewCliImportResult,
  type WorkflowBoardStage,
  type WorkflowCardId,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import { AlertTriangleIcon, InfoIcon, RefreshCwIcon, ShieldAlertIcon } from "lucide-react";
import { useEffect, useId, useRef, useState, type FormEvent } from "react";

import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Alert, AlertDescription, AlertTitle } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Checkbox } from "../ui/checkbox";
import {
  Dialog,
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPanel,
  DialogPopup,
  DialogTitle,
} from "../ui/dialog";
import { Input } from "../ui/input";
import {
  formatArgvTokens,
  isWorkflowBoardWorkspaceStrategyValid,
  normalizeWorkspaceStrategy,
  resolveDefaultBoardLaunchConfig,
} from "./workflowBoard.logic";
import { WorkflowBoardLaunchControls } from "./WorkflowBoardLaunchControls";

const DEFAULT_CLI_DIR = ".legion-cli";

export function WorkflowBoardCliImportDialog(props: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly environmentId: EnvironmentId;
  readonly projectId: ProjectId;
  readonly providers: ReadonlyArray<ServerProvider>;
  readonly settings: ServerSettings;
  readonly existingWorktreePaths: ReadonlyArray<string>;
  readonly onImported: (cardId: WorkflowCardId, stage: WorkflowBoardStage) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const cliDirHintId = useId();
  const acknowledgePoliciesId = useId();
  const unsupportedPoliciesTitleId = useId();

  const canPreview = useAtomValue(
    workflowBoard.previewCliImport.permissionAtom(props.environmentId),
  );
  const canApply = useAtomValue(workflowBoard.applyCliImport.permissionAtom(props.environmentId));
  const previewCliImport = useAtomCommand(workflowBoard.previewCliImport, {
    label: "workflow board preview cli import",
    reportFailure: false,
  });
  const applyCliImport = useAtomCommand(workflowBoard.applyCliImport, {
    label: "workflow board apply cli import",
    reportFailure: false,
  });

  const defaults = resolveDefaultBoardLaunchConfig({
    providers: props.providers,
    settings: props.settings,
    projectId: props.projectId,
  });

  const previewSeqRef = useRef(0);

  const [cliDir, setCliDir] = useState(DEFAULT_CLI_DIR);
  const [openGeneration, setOpenGeneration] = useState(0);
  const [settledAutoPreviewKey, setSettledAutoPreviewKey] = useState<string | null>(null);
  const [previewState, setPreviewState] = useState<{
    readonly cliDir: string;
    readonly result: WorkflowBoardPreviewCliImportResult;
  } | null>(null);
  const [manualPendingCliDir, setManualPendingCliDir] = useState<string | null>(null);
  const [applying, setApplying] = useState(false);
  const [previewErrorState, setPreviewErrorState] = useState<{
    readonly cliDir: string;
    readonly cause: unknown;
  } | null>(null);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [acknowledgePolicies, setAcknowledgePolicies] = useState(false);
  const [modelSelection, setModelSelection] = useState<ModelSelection | null>(null);
  const [runtimeMode, setRuntimeMode] = useState(defaults.runtimeMode);
  const [interactionMode, setInteractionMode] = useState(defaults.interactionMode);
  const [workspaceStrategy, setWorkspaceStrategy] = useState(defaults.workspaceStrategy);

  const trimmedCliDir = cliDir.trim();
  const normalizedCliDir = trimmedCliDir || DEFAULT_CLI_DIR;
  const cliDirValid = isValidCliImportDirInput(cliDir);
  const autoPreviewKey =
    props.open && canPreview ? `${props.environmentId}:${props.projectId}:${openGeneration}` : null;
  const isInitialAutoPreviewPending =
    autoPreviewKey !== null &&
    settledAutoPreviewKey !== autoPreviewKey &&
    manualPendingCliDir === null;
  const pendingCliDir =
    manualPendingCliDir ?? (isInitialAutoPreviewPending ? DEFAULT_CLI_DIR : null);
  const previewing = pendingCliDir !== null;
  const previewingCurrentDir = pendingCliDir === normalizedCliDir;
  const previewedCliDir = previewState?.cliDir ?? null;
  const preview =
    previewState !== null && previewState.cliDir === normalizedCliDir ? previewState.result : null;
  const previewErrorCause =
    previewErrorState !== null && previewErrorState.cliDir === normalizedCliDir
      ? previewErrorState.cause
      : null;
  const previewError =
    previewErrorCause === null
      ? null
      : (workflowBoardErrorMessage(previewErrorCause, messages) ??
        gt("Could not preview CLI project."));
  const effectiveModelSelection = modelSelection ?? defaults.modelSelection;
  const workspaceStrategyValid = isWorkflowBoardWorkspaceStrategyValid(workspaceStrategy);

  const blockingUnacknowledgedPolicies = (preview?.unsupportedPolicies ?? []).filter(
    (policy) => policy.blocksExecution && policy.status !== "acknowledged",
  );

  // oxlint-disable-next-line legioncode/no-unreasoned-state-sync -- fetches live .legion-cli directory preview from server on dialog open because disk lock files change outside WebSocket state
  useEffect(() => {
    if (autoPreviewKey === null) return;
    let active = true;
    const seq = ++previewSeqRef.current;
    const targetCliDir = DEFAULT_CLI_DIR;
    void previewCliImport({
      environmentId: props.environmentId,
      input: {
        projectId: props.projectId,
        cliDir: WorkflowBoardSafeInputPath.make(targetCliDir),
      },
    }).then((result) => {
      if (!active || previewSeqRef.current !== seq) return;
      setSettledAutoPreviewKey(autoPreviewKey);
      setApplyError(null);
      setAcknowledgePolicies(false);
      if (result._tag === "Failure") {
        setPreviewState(null);
        setPreviewErrorState({ cliDir: targetCliDir, cause: Cause.squash(result.cause) });
        return;
      }
      setPreviewErrorState(null);
      setPreviewState({ cliDir: targetCliDir, result: result.value });
    });
    return () => {
      active = false;
    };
  }, [autoPreviewKey, props.environmentId, props.projectId, previewCliImport]);

  const handleCliDirChange = (nextValue: string) => {
    setCliDir(nextValue);
    const nextNormalized = nextValue.trim() || DEFAULT_CLI_DIR;
    if (previewState !== null && previewState.cliDir !== nextNormalized) {
      setPreviewState(null);
      setAcknowledgePolicies(false);
    }
    if (previewErrorState !== null && previewErrorState.cliDir !== nextNormalized) {
      setPreviewErrorState(null);
    }
    setApplyError(null);
  };

  const closeAndReset = () => {
    previewSeqRef.current += 1;
    setOpenGeneration((prev) => prev + 1);
    setSettledAutoPreviewKey(null);
    setCliDir(DEFAULT_CLI_DIR);
    setPreviewState(null);
    setManualPendingCliDir(null);
    setPreviewErrorState(null);
    setApplyError(null);
    setAcknowledgePolicies(false);
    setModelSelection(null);
    setRuntimeMode(defaults.runtimeMode);
    setInteractionMode(defaults.interactionMode);
    setWorkspaceStrategy(defaults.workspaceStrategy);
    props.onOpenChange(false);
  };

  const handleDialogOpenChange = (nextOpen: boolean) => {
    if (!nextOpen) {
      if (applying) return;
      closeAndReset();
      return;
    }
    props.onOpenChange(true);
  };

  const handlePreview = async () => {
    const targetCliDir = normalizedCliDir;
    if (!canPreview || applying || !cliDirValid || pendingCliDir === targetCliDir) return;
    const seq = ++previewSeqRef.current;
    setSettledAutoPreviewKey(autoPreviewKey);
    setManualPendingCliDir(targetCliDir);
    setPreviewErrorState(null);
    setApplyError(null);
    setAcknowledgePolicies(false);

    const result = await previewCliImport({
      environmentId: props.environmentId,
      input: {
        projectId: props.projectId,
        ...(trimmedCliDir !== "" ? { cliDir: WorkflowBoardSafeInputPath.make(trimmedCliDir) } : {}),
      },
    });
    if (previewSeqRef.current !== seq) return;
    setManualPendingCliDir(null);

    if (result._tag === "Failure") {
      setPreviewState(null);
      setPreviewErrorState({ cliDir: targetCliDir, cause: Cause.squash(result.cause) });
      return;
    }

    setPreviewErrorState(null);
    setPreviewState({ cliDir: targetCliDir, result: result.value });
  };

  const handleApply = async (event: FormEvent) => {
    event.preventDefault();
    if (
      !canApply ||
      applying ||
      previewing ||
      !cliDirValid ||
      !workspaceStrategyValid ||
      normalizedCliDir !== previewedCliDir ||
      preview === null ||
      !preview.canApply
    ) {
      return;
    }

    setApplying(true);
    setApplyError(null);

    let result;
    try {
      const shouldAcknowledge = acknowledgePolicies && blockingUnacknowledgedPolicies.length > 0;
      result = await applyCliImport({
        environmentId: props.environmentId,
        input: {
          projectId: props.projectId,
          ...(trimmedCliDir !== ""
            ? { cliDir: WorkflowBoardSafeInputPath.make(trimmedCliDir) }
            : {}),
          ...(shouldAcknowledge
            ? {
                acknowledgeUnsupportedPolicies: true,
                acknowledgedPolicies: blockingUnacknowledgedPolicies.map((policy) => policy.key),
              }
            : {}),
          ...(effectiveModelSelection !== null ? { modelSelection: effectiveModelSelection } : {}),
          runtimeMode,
          interactionMode,
          workspaceStrategy: normalizeWorkspaceStrategy(workspaceStrategy),
        },
      });
    } catch (error) {
      setApplying(false);
      setApplyError(
        workflowBoardErrorMessage(error, messages) ?? gt("Could not import CLI project."),
      );
      return;
    }

    setApplying(false);
    if (result._tag === "Failure") {
      setApplyError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not import CLI project."),
      );
      return;
    }

    closeAndReset();
    props.onImported(result.value.cardId, result.value.card.stage);
  };

  const applyDisabled =
    !canApply ||
    applying ||
    previewing ||
    !cliDirValid ||
    !workspaceStrategyValid ||
    normalizedCliDir !== previewedCliDir ||
    preview === null ||
    !preview.canApply;

  return (
    <Dialog
      open={props.open}
      onOpenChange={handleDialogOpenChange}
      disablePointerDismissal={applying}
    >
      <DialogPopup className="max-w-3xl" showCloseButton={!applying}>
        <form
          className="flex min-h-0 flex-1 flex-col"
          onSubmit={(event) => void handleApply(event)}
        >
          <DialogHeader>
            <DialogTitle>
              <T>Import CLI project</T>
            </DialogTitle>
            <DialogDescription>
              <T>
                Preview an existing .legion-cli directory, review historical receipts and adopted
                policies, and import explicitly into the project workflow board.
              </T>
            </DialogDescription>
          </DialogHeader>

          <DialogPanel>
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="workflow-cli-import-dir"
                className="text-xs font-medium text-foreground"
              >
                <T>CLI directory (relative to workspace)</T>
              </label>
              <div className="flex flex-wrap items-center gap-2">
                <Input
                  id="workflow-cli-import-dir"
                  font="mono"
                  aria-invalid={!cliDirValid || undefined}
                  aria-describedby={!cliDirValid ? cliDirHintId : undefined}
                  placeholder={gt(".legion-cli", {
                    $context: "workflow board cli import directory placeholder",
                  })}
                  value={cliDir}
                  onValueChange={handleCliDirChange}
                  disabled={applying}
                  className="min-w-0 flex-1"
                />
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={!canPreview || applying || !cliDirValid || previewingCurrentDir}
                  onClick={() => void handlePreview()}
                >
                  <RefreshCwIcon />
                  {previewingCurrentDir ? <T>Previewing…</T> : <T>Preview import</T>}
                </Button>
              </div>
              {!cliDirValid ? (
                <p id={cliDirHintId} role="alert" className="text-xs text-destructive-foreground">
                  <T>
                    Enter a safe workspace-relative path (no traversal, .git, or credential files).
                  </T>
                </p>
              ) : null}
            </div>

            {previewError !== null ? (
              <Alert variant="error">
                <AlertTriangleIcon />
                <AlertTitle>
                  <T>Preview failed</T>
                </AlertTitle>
                <AlertDescription>{previewError}</AlertDescription>
              </Alert>
            ) : null}

            {applyError !== null ? (
              <Alert variant="error">
                <AlertTriangleIcon />
                <AlertTitle>
                  <T>Import failed</T>
                </AlertTitle>
                <AlertDescription>{applyError}</AlertDescription>
              </Alert>
            ) : null}

            {preview !== null ? (
              <div className="flex flex-col gap-4">
                {!preview.canApply || preview.activeLocks.length > 0 ? (
                  <Alert variant="error" data-cli-import-refusal="active-lock">
                    <ShieldAlertIcon />
                    <AlertTitle>
                      <T>Active CLI run detected: takeover refused</T>
                    </AlertTitle>
                    <AlertDescription>
                      <p>
                        {preview.refusalReason ??
                          gt("Stop the active CLI run before importing into LegionCode.")}
                      </p>
                      {preview.activeLocks.length > 0 ? (
                        <ul className="mt-2 flex flex-col gap-1 font-mono text-2xs">
                          {preview.activeLocks.map((lock) => (
                            <li key={`${lock.kind}:${lock.relativePath}`}>
                              <Var>{lock.kind}</Var> · <Var>{lock.relativePath}</Var>
                              {lock.pid !== null ? (
                                <T>
                                  {" "}
                                  (pid <Var>{lock.pid}</Var>)
                                </T>
                              ) : null}
                              {lock.runId !== null ? (
                                <T>
                                  {" "}
                                  · run <Var>{lock.runId}</Var>
                                </T>
                              ) : null}
                              : <Var>{lock.message}</Var>
                            </li>
                          ))}
                        </ul>
                      ) : null}
                    </AlertDescription>
                  </Alert>
                ) : null}

                {preview.staleLocks.length > 0 ? (
                  <Alert variant="info" role="note">
                    <InfoIcon />
                    <AlertTitle>
                      <T>Stale CLI locks ignored</T>
                    </AlertTitle>
                    <AlertDescription>
                      <ul className="flex flex-col gap-1 font-mono text-2xs">
                        {preview.staleLocks.map((lock) => (
                          <li key={`${lock.kind}:${lock.relativePath}`}>
                            <Var>{lock.relativePath}</Var>
                            {lock.pid !== null ? (
                              <T>
                                {" "}
                                (dead pid <Var>{lock.pid}</Var>)
                              </T>
                            ) : null}
                            : <Var>{lock.message}</Var>
                          </li>
                        ))}
                      </ul>
                    </AlertDescription>
                  </Alert>
                ) : null}

                {preview.warnings.length > 0 ? (
                  <Alert variant="warning" role="note">
                    <AlertTriangleIcon />
                    <AlertTitle>
                      <T>Import notes</T>
                    </AlertTitle>
                    <AlertDescription>
                      <ul className="flex flex-col gap-1 text-xs">
                        {preview.warnings.map((warning) => (
                          <li key={warning}>{warning}</li>
                        ))}
                      </ul>
                    </AlertDescription>
                  </Alert>
                ) : null}

                {preview.unsupportedPolicies.length > 0 ? (
                  <Alert
                    variant="warning"
                    role="region"
                    aria-labelledby={unsupportedPoliciesTitleId}
                    data-cli-import-policies="true"
                  >
                    <AlertTriangleIcon />
                    <AlertTitle id={unsupportedPoliciesTitleId}>
                      <T>
                        Unsupported adopted CLI policies (
                        <Var>{preview.unsupportedPolicies.length}</Var>)
                      </T>
                    </AlertTitle>
                    <AlertDescription>
                      <ul className="mt-1 flex flex-col gap-2">
                        {preview.unsupportedPolicies.map((policy) => (
                          <li
                            key={policy.key}
                            className="flex flex-col gap-1 rounded border bg-background/80 p-2 text-xs"
                          >
                            <div className="flex flex-wrap items-center justify-between gap-2">
                              <div className="flex flex-wrap items-center gap-1.5">
                                <span className="font-mono font-semibold text-foreground">
                                  {policy.key}
                                </span>
                                <Badge variant="outline" size="sm">
                                  {policy.category}
                                </Badge>
                                <Badge
                                  variant={policy.status === "acknowledged" ? "success" : "warning"}
                                  size="sm"
                                >
                                  {workflowBoardCliImportPolicyStatusLabel(policy.status, messages)}
                                </Badge>
                                {policy.blocksExecution ? (
                                  <Badge variant="error" size="sm">
                                    <T>Blocks Execute</T>
                                  </Badge>
                                ) : null}
                              </div>
                              <span className="font-mono text-2xs text-muted-foreground">
                                {policy.sourcePath}
                              </span>
                            </div>
                            <p className="text-foreground">{policy.summary}</p>
                            {policy.configuredValue !== null ? (
                              <p className="font-mono text-2xs text-muted-foreground">
                                <T>
                                  Configured: <Var>{policy.configuredValue}</Var>
                                </T>
                              </p>
                            ) : null}
                          </li>
                        ))}
                      </ul>
                      {blockingUnacknowledgedPolicies.length > 0 ? (
                        <label
                          htmlFor={acknowledgePoliciesId}
                          className="mt-3 flex cursor-pointer items-center gap-2 text-xs text-foreground"
                        >
                          <Checkbox
                            id={acknowledgePoliciesId}
                            checked={acknowledgePolicies}
                            onCheckedChange={(checked) => setAcknowledgePolicies(Boolean(checked))}
                            disabled={applying}
                          />
                          <span>
                            <T>
                              Acknowledge blocking unsupported policies during import so Execute can
                              run in LegionCode
                            </T>
                          </span>
                        </label>
                      ) : null}
                    </AlertDescription>
                  </Alert>
                ) : null}

                <section className="flex flex-col gap-2 rounded-lg border bg-card p-3 text-xs">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <h3 className="text-sm font-semibold text-foreground">{preview.title}</h3>
                      <Badge variant="outline" size="sm">
                        {preview.specId}
                      </Badge>
                      <Badge variant="outline" size="sm">
                        {workflowBoardStageLabel(preview.stage, messages)}
                      </Badge>
                      <Badge
                        variant={preview.status === "blocked" ? "error" : "secondary"}
                        size="sm"
                      >
                        {workflowBoardCardStatusLabel(preview.status, messages)}
                      </Badge>
                      {preview.alreadyImported ? (
                        <Badge variant="info" size="sm">
                          <T>Already imported (re-syncs idempotently)</T>
                        </Badge>
                      ) : null}
                    </div>
                    {preview.existingCardId !== null ? (
                      <span className="font-mono text-2xs text-muted-foreground">
                        {preview.existingCardId}
                      </span>
                    ) : null}
                  </div>

                  <p className="font-mono text-2xs text-muted-foreground">{preview.sourcePath}</p>

                  <div className="grid gap-2 sm:grid-cols-3">
                    <div className="rounded border bg-background p-2">
                      <span className="text-muted-foreground">
                        <T>Decisions</T>
                      </span>
                      <p className="font-mono font-semibold text-foreground">
                        {preview.decisionIds.length}
                      </p>
                    </div>
                    <div className="rounded border bg-background p-2">
                      <span className="text-muted-foreground">
                        <T>Assumptions</T>
                      </span>
                      <p className="font-mono font-semibold text-foreground">
                        {preview.assumptionIds.length}
                      </p>
                    </div>
                    <div className="rounded border bg-background p-2">
                      <span className="text-muted-foreground">
                        <T>Acceptance criteria</T>
                      </span>
                      <p className="font-mono font-semibold text-foreground">
                        {preview.acceptanceCriterionIds.length}
                      </p>
                    </div>
                  </div>
                </section>

                <section className="flex flex-col gap-2 rounded-lg border bg-card p-3 text-xs">
                  <div className="flex items-center justify-between gap-2">
                    <h3 className="font-semibold text-foreground">
                      <T>
                        Preserved tasks (<Var>{preview.tasks.length}</Var>)
                      </T>
                    </h3>
                  </div>
                  {preview.tasks.length === 0 ? (
                    <p className="text-muted-foreground">
                      <T>No CLI tasks found.</T>
                    </p>
                  ) : (
                    <ul className="flex flex-col gap-2">
                      {preview.tasks.map((task) => (
                        <li
                          key={task.taskId}
                          data-cli-preview-task={task.taskId}
                          className="flex flex-col gap-1 rounded border bg-background p-2"
                        >
                          <div className="flex flex-wrap items-center justify-between gap-2">
                            <div className="flex flex-wrap items-center gap-2">
                              <span className="font-mono font-semibold text-foreground">
                                {task.taskId}
                              </span>
                              <span className="text-foreground">{task.title}</span>
                            </div>
                            <Badge
                              variant={task.status === "blocked" ? "error" : "secondary"}
                              size="sm"
                            >
                              {task.status}
                            </Badge>
                          </div>
                          {task.blockerSummary ? (
                            <p className="text-destructive-foreground">{task.blockerSummary}</p>
                          ) : null}
                          {task.verificationCommands.length > 0 ? (
                            <p className="font-mono text-2xs text-muted-foreground">
                              {task.verificationCommands.map(formatArgvTokens).join(" ; ")}
                            </p>
                          ) : null}
                          {task.missingLogNotes.map((note) => (
                            <p
                              key={`${note.relativePath}:${note.note}`}
                              className="text-warning-foreground"
                            >
                              <span className="font-mono">{note.relativePath}</span>: {note.note}
                            </p>
                          ))}
                        </li>
                      ))}
                    </ul>
                  )}
                </section>

                {preview.historicalReceipts.length > 0 ? (
                  <section className="flex flex-col gap-2 rounded-lg border bg-card p-3 text-xs">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <h3 className="font-semibold text-foreground">
                        <T>
                          Historical receipts (<Var>{preview.historicalReceipts.length}</Var>)
                        </T>
                      </h3>
                      <Badge variant="secondary" size="sm">
                        <T>Requires native revalidation</T>
                      </Badge>
                    </div>
                    <ul className="flex flex-col gap-1.5">
                      {preview.historicalReceipts.map((receipt) => (
                        <li
                          key={receipt.id}
                          className="flex flex-col gap-1 rounded border bg-background p-2"
                        >
                          <div className="flex flex-wrap items-center justify-between gap-2">
                            <div className="flex flex-wrap items-center gap-1.5">
                              <Badge variant="outline" size="sm">
                                {workflowBoardCliImportHistoricalReceiptKindLabel(
                                  receipt.kind,
                                  messages,
                                )}
                              </Badge>
                              <Badge variant="secondary" size="sm">
                                <T>Historical</T>
                              </Badge>
                            </div>
                            <span className="font-mono text-2xs text-muted-foreground">
                              {receipt.sourcePath}
                            </span>
                          </div>
                          <p className="text-foreground">{receipt.summary}</p>
                        </li>
                      ))}
                    </ul>
                  </section>
                ) : null}

                {preview.missingLogNotes.length > 0 ? (
                  <section className="flex flex-col gap-1.5 rounded-lg border bg-card p-3 text-xs">
                    <h3 className="font-semibold text-foreground">
                      <T>Missing check logs noted</T>
                    </h3>
                    <ul className="flex flex-col gap-1">
                      {preview.missingLogNotes.map((note) => (
                        <li
                          key={`${note.taskId ?? ""}:${note.relativePath}`}
                          className="text-muted-foreground"
                        >
                          <span className="font-mono text-foreground">{note.relativePath}</span>:{" "}
                          {note.note}
                        </li>
                      ))}
                    </ul>
                  </section>
                ) : null}
              </div>
            ) : null}

            <WorkflowBoardLaunchControls
              environmentId={props.environmentId}
              projectId={props.projectId}
              providers={props.providers}
              settings={props.settings}
              existingWorktreePaths={props.existingWorktreePaths}
              modelSelection={effectiveModelSelection}
              onModelSelectionChange={setModelSelection}
              runtimeMode={runtimeMode}
              onRuntimeModeChange={setRuntimeMode}
              interactionMode={interactionMode}
              onInteractionModeChange={setInteractionMode}
              workspaceStrategy={workspaceStrategy}
              onWorkspaceStrategyChange={setWorkspaceStrategy}
              disabled={applying}
            />
          </DialogPanel>

          <DialogFooter>
            <DialogClose render={<Button type="button" variant="outline" disabled={applying} />}>
              <T>Cancel</T>
            </DialogClose>
            <Button type="submit" disabled={applyDisabled}>
              {applying ? <T>Importing…</T> : <T>Apply import</T>}
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}

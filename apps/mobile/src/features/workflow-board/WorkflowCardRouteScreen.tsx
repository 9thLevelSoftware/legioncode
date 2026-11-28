import { useAtomValue } from "@effect/atom-react";
import { isAtomCommandInterrupted } from "@legioncode/client-runtime/state/runtime";
import {
  EnvironmentId,
  PositiveInt,
  ProjectId,
  ThreadId,
  WorkflowBoardArtifactKey,
  WorkflowCardId,
  type WorkflowBoardFindingResolveDisposition,
  type WorkflowBoardManualAcceptanceVerdict,
  type WorkflowBoardReturnTargetStage,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import { StackActions, useNavigation, type StaticScreenProps } from "@react-navigation/native";
import * as Cause from "effect/Cause";
import * as Option from "effect/Option";
import * as Schema from "effect/Schema";
import { T, useGT, useMessages, Var } from "gt-react-native";
import { useRef, useState } from "react";
import { View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { StatusPill } from "../../components/StatusPill";
import { AppBar } from "../../components/ui/AppBar";
import { AppText as Text, AppTextInput } from "../../components/ui/AppText";
import { Button } from "../../components/ui/Button";
import { Chip } from "../../components/ui/Chip";
import * as Dialog from "../../components/ui/Dialog";
import { EmptyState } from "../../components/ui/EmptyState";
import { Notice } from "../../components/ui/Notice";
import { ScreenContent } from "../../components/ui/ScreenContent";
import { ScreenScrollView as ScrollView } from "../../components/ui/ScreenScrollView";
import { SegmentedControl } from "../../components/ui/SegmentedControl";
import { useProjects } from "../../state/entities";
import { useEnvironments } from "../../state/environments";
import { useEnvironmentQuery } from "../../state/query";
import { startOpeningEnvironmentThread } from "../../state/threads";
import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { openSettings } from "../settings/settings-navigation";
import {
  ExecuteInspectSection,
  HistoryInspectSection,
  PlanInspectSection,
  ShipInspectSection,
  SpecInspectSection,
} from "./WorkflowCardInspectSections";
import {
  MOBILE_CARD_SUB_TABS,
  collectDownstreamInvalidationStates,
  collectMobileCardThreadLinks,
  defaultMobileCardSubTab,
  mobileCardSubTabLabel,
  mobileCardThreadLinkLabel,
  mobileReturnTargetStageLabel,
  resolveMobileWorkflowBoardTarget,
  validReturnTargetStages,
  workflowBoardAvailabilityNotice,
  workflowBoardCardBlocker,
  workflowBoardCardNextAction,
  workflowBoardCardStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardNeedsRecovery,
  workflowBoardProjectNotFoundNotice,
  workflowBoardReadPermissionNotice,
  workflowBoardStageLabel,
  workflowBoardStaleEvidenceSummary,
  workflowBoardUnacknowledgedBlockingPolicies,
  workflowCardStateKey,
  type MobileCardSubTab,
} from "./workflowBoard.logic";

export type WorkflowCardRouteParams = {
  readonly environmentId: string;
  readonly projectId: string;
  readonly cardId: string;
  readonly stage?: string | undefined;
};

const decodeEnvironmentId = Schema.decodeUnknownOption(EnvironmentId);
const decodeProjectId = Schema.decodeUnknownOption(ProjectId);
const decodeWorkflowCardId = Schema.decodeUnknownOption(WorkflowCardId);
const decodeWorkflowBoardArtifactKey = Schema.decodeUnknownOption(WorkflowBoardArtifactKey);

function WorkflowCardBody(props: {
  readonly environmentId: EnvironmentId;
  readonly projectId: ProjectId;
  readonly card: WorkflowCardDetail;
  readonly requestedStage?: string | undefined;
  readonly hasPairedEnvironment: boolean;
}) {
  const gt = useGT();
  const messages = useMessages();
  const navigation = useNavigation();
  const { card } = props;

  const canReadBoard = useAtomValue(workflowBoard.readPermissionAtom(props.environmentId));
  const canApproveSpec = useAtomValue(
    workflowBoard.approveSpec.permissionAtom(props.environmentId),
  );
  const canApprovePlan = useAtomValue(
    workflowBoard.approvePlan.permissionAtom(props.environmentId),
  );
  const canApproveDelivery = useAtomValue(
    workflowBoard.approveDelivery.permissionAtom(props.environmentId),
  );
  const canPauseCard = useAtomValue(workflowBoard.pauseCard.permissionAtom(props.environmentId));
  const canResumeCard = useAtomValue(workflowBoard.resumeCard.permissionAtom(props.environmentId));
  const canRetryStep = useAtomValue(workflowBoard.retryStep.permissionAtom(props.environmentId));
  const canReturn = useAtomValue(
    workflowBoard.returnForRevision.permissionAtom(props.environmentId),
  );
  const canRecordAcceptance = useAtomValue(
    workflowBoard.recordManualAcceptance.permissionAtom(props.environmentId),
  );
  const canResolveFinding = useAtomValue(
    workflowBoard.resolveFinding.permissionAtom(props.environmentId),
  );

  const approveSpec = useAtomCommand(workflowBoard.approveSpec, {
    label: "workflow board approve spec",
    reportFailure: false,
  });
  const approvePlan = useAtomCommand(workflowBoard.approvePlan, {
    label: "workflow board approve plan",
    reportFailure: false,
  });
  const approveDelivery = useAtomCommand(workflowBoard.approveDelivery, {
    label: "workflow board approve delivery",
    reportFailure: false,
  });
  const pauseCard = useAtomCommand(workflowBoard.pauseCard, {
    label: "workflow board pause card",
    reportFailure: false,
  });
  const resumeCard = useAtomCommand(workflowBoard.resumeCard, {
    label: "workflow board resume card",
    reportFailure: false,
  });
  const retryStep = useAtomCommand(workflowBoard.retryStep, {
    label: "workflow board retry step",
    reportFailure: false,
  });
  const returnForRevision = useAtomCommand(workflowBoard.returnForRevision, {
    label: "workflow board return for revision",
    reportFailure: false,
  });
  const recordManualAcceptance = useAtomCommand(workflowBoard.recordManualAcceptance, {
    label: "workflow board record manual acceptance",
    reportFailure: false,
  });
  const resolveFinding = useAtomCommand(workflowBoard.resolveFinding, {
    label: "workflow board resolve finding",
    reportFailure: false,
  });

  const [selectedSubTab, setSelectedSubTab] = useState<MobileCardSubTab | null>(null);
  const [selectedArtifactKey, setSelectedArtifactKey] = useState<string | null>(null);
  const [returnStage, setReturnStage] = useState<WorkflowBoardReturnTargetStage | null>(null);
  const [returnReason, setReturnReason] = useState("");
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const actingRef = useRef(false);
  const openDialogRef = useRef(false);

  const activeSubTab = selectedSubTab ?? defaultMobileCardSubTab(card, props.requestedStage);
  const typedArtifactKey =
    selectedArtifactKey !== null
      ? Option.getOrNull(decodeWorkflowBoardArtifactKey(selectedArtifactKey))
      : null;

  const artifactQuery = useEnvironmentQuery(
    canReadBoard && activeSubTab === "spec" && typedArtifactKey !== null
      ? workflowBoard.artifact({
          environmentId: props.environmentId,
          input: {
            cardId: card.cardId,
            key: typedArtifactKey,
          },
        })
      : null,
  );
  const cardDiffQuery = useEnvironmentQuery(
    canReadBoard && (activeSubTab === "execute" || activeSubTab === "ship")
      ? workflowBoard.cardDiff({
          environmentId: props.environmentId,
          input: { cardId: card.cardId, includePatches: true },
        })
      : null,
  );

  const runGuarded = async (actionKey: string, work: () => Promise<void>) => {
    if (actingRef.current || openDialogRef.current) return;
    actingRef.current = true;
    setBusyAction(actionKey);
    setActionError(null);
    setActionNotice(null);
    await work().finally(() => {
      actingRef.current = false;
      setBusyAction(null);
    });
  };

  const confirmGuarded = (request: Dialog.ConfirmDialogRequest) => {
    if (actingRef.current || openDialogRef.current) return;
    openDialogRef.current = true;
    Dialog.confirm({
      ...request,
      onConfirm: () => {
        openDialogRef.current = false;
        request.onConfirm();
      },
      onCancel: () => {
        openDialogRef.current = false;
        request.onCancel?.();
      },
    });
  };

  const handleThreadPress = (threadId: ThreadId | null) => {
    if (threadId === null || !props.hasPairedEnvironment) return;
    startOpeningEnvironmentThread(props.environmentId, threadId);
    navigation.dispatch(
      StackActions.push("Thread", {
        environmentId: String(props.environmentId),
        threadId: String(threadId),
      }),
    );
  };

  const handleApproveSpec = () => {
    confirmGuarded({
      title: gt("Approve spec?"),
      message: gt("Approves revision {revision} so Plan can start.", { revision: card.revision }),
      confirmText: gt("Approve spec"),
      onConfirm: () => {
        void runGuarded("approve-spec", async () => {
          const result = await approveSpec({
            environmentId: props.environmentId,
            input: {
              cardId: card.cardId,
              expectedRevision: PositiveInt.make(card.revision),
              ...(card.spec.specDigest !== null ? { expectedDigest: card.spec.specDigest } : {}),
            },
          });
          if (result._tag === "Failure") {
            if (isAtomCommandInterrupted(result)) return;
            setActionError(
              workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
                gt("Could not approve spec."),
            );
            return;
          }
          setActionNotice(gt("Spec approved."));
        });
      },
    });
  };

  const handleApprovePlan = () => {
    confirmGuarded({
      title: gt("Approve plan?"),
      message: gt("Approves the plan, tasks, and checks for execution."),
      confirmText: gt("Approve plan"),
      onConfirm: () => {
        void runGuarded("approve-plan", async () => {
          const result = await approvePlan({
            environmentId: props.environmentId,
            input: {
              cardId: card.cardId,
              expectedRevision: PositiveInt.make(card.revision),
              ...(card.spec.specDigest !== null
                ? { expectedSpecDigest: card.spec.specDigest }
                : {}),
              ...(card.plan?.planDigest !== undefined && card.plan.planDigest !== null
                ? { expectedPlanDigest: card.plan.planDigest }
                : {}),
            },
          });
          if (result._tag === "Failure") {
            if (isAtomCommandInterrupted(result)) return;
            setActionError(
              workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
                gt("Could not approve plan."),
            );
            return;
          }
          setActionNotice(gt("Plan approved."));
        });
      },
    });
  };

  const handleApproveDelivery = (note: string) => {
    const snapshotDigest =
      card.shipReadiness?.productSnapshotDigest ??
      cardDiffQuery.data?.productSnapshotDigest ??
      null;
    confirmGuarded({
      title: gt("Approve delivery?"),
      message: gt("Marks the card shipped without auto-committing changes."),
      confirmText: gt("Approve delivery"),
      onConfirm: () => {
        void runGuarded("approve-delivery", async () => {
          const trimmedNote = note.trim();
          const result = await approveDelivery({
            environmentId: props.environmentId,
            input: {
              cardId: card.cardId,
              expectedRevision: PositiveInt.make(card.revision),
              ...(snapshotDigest !== null ? { expectedProductSnapshotDigest: snapshotDigest } : {}),
              ...(trimmedNote !== "" ? { note: trimmedNote } : {}),
            },
          });
          if (result._tag === "Failure") {
            if (isAtomCommandInterrupted(result)) return;
            setActionError(
              workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
                gt("Could not approve delivery."),
            );
            return;
          }
          setActionNotice(gt("Delivery approved. Card marked shipped."));
        });
      },
    });
  };

  const handlePause = () => {
    void runGuarded("pause", async () => {
      const result = await pauseCard({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
        },
      });
      if (result._tag === "Failure") {
        if (isAtomCommandInterrupted(result)) return;
        setActionError(
          workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
            gt("Could not pause card."),
        );
        return;
      }
      setActionNotice(gt("Card paused."));
    });
  };

  const handleResume = (reconcileInterrupted: boolean) => {
    void runGuarded(reconcileInterrupted ? "reconcile-resume" : "resume", async () => {
      const result = await resumeCard({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
          ...(reconcileInterrupted ? { reconcileInterrupted: true } : {}),
        },
      });
      if (result._tag === "Failure") {
        if (isAtomCommandInterrupted(result)) return;
        setActionError(
          workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
            gt("Could not resume card."),
        );
        return;
      }
      setActionNotice(
        reconcileInterrupted
          ? gt("Reconciled {count} item(s) and resumed card.", {
              count: result.value.reconciledCount,
            })
          : gt("Card resumed."),
      );
    });
  };

  const handleRetryStep = (stepKind: "task" | "check" | "review" | "stage", targetId?: string) => {
    const trimmedTargetId = targetId?.trim();
    if ((stepKind === "task" || stepKind === "check") && !trimmedTargetId) {
      setActionError(gt("Could not retry step."));
      return;
    }
    const key = trimmedTargetId ? `retry:${stepKind}:${trimmedTargetId}` : `retry:${stepKind}`;
    void runGuarded(key, async () => {
      const result = await retryStep({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
          stepKind,
          ...(trimmedTargetId !== undefined && trimmedTargetId !== ""
            ? { targetId: trimmedTargetId }
            : {}),
        },
      });
      if (result._tag === "Failure") {
        if (isAtomCommandInterrupted(result)) return;
        setActionError(
          workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
            gt("Could not retry step."),
        );
        return;
      }
      setActionNotice(gt("Step retry submitted."));
    });
  };

  const handleResolveFinding = (input: {
    readonly findingId: string;
    readonly disposition: WorkflowBoardFindingResolveDisposition;
    readonly reason: string;
  }) => {
    const trimmedReason = input.reason.trim();
    if (
      (input.disposition === "dismiss" || input.disposition === "accept_risk") &&
      trimmedReason === ""
    ) {
      setActionError(gt("A reason is required when dismissing or accepting risk."));
      return;
    }
    void runGuarded(`finding:${input.findingId}`, async () => {
      const result = await resolveFinding({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
          findingId: input.findingId,
          disposition: input.disposition,
          ...(trimmedReason !== "" ? { reason: trimmedReason } : {}),
          ...(card.reviewReport?.reviewId ? { reviewId: card.reviewReport.reviewId } : {}),
        },
      });
      if (result._tag === "Failure") {
        if (isAtomCommandInterrupted(result)) return;
        setActionError(
          workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
            gt("Could not resolve finding."),
        );
        return;
      }
      setActionNotice(gt("Finding updated."));
    });
  };

  const handleRecordAcceptance = (input: {
    readonly criterionId: string;
    readonly verdict: WorkflowBoardManualAcceptanceVerdict;
    readonly note: string;
    readonly evidenceRef: string;
  }) => {
    const trimmedNote = input.note.trim();
    if ((input.verdict === "fail" || input.verdict === "not-applicable") && trimmedNote === "") {
      setActionError(gt("A note is required when recording Fail or Not applicable."));
      return;
    }
    const snapshotDigest =
      card.shipReadiness?.productSnapshotDigest ??
      cardDiffQuery.data?.productSnapshotDigest ??
      null;
    void runGuarded(`ac:${input.criterionId}`, async () => {
      const trimmedRef = input.evidenceRef.trim();
      const result = await recordManualAcceptance({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
          criterionId: input.criterionId,
          verdict: input.verdict,
          ...(trimmedNote !== "" ? { note: trimmedNote } : {}),
          ...(trimmedRef !== "" ? { evidenceRef: trimmedRef } : {}),
          ...(snapshotDigest !== null ? { expectedProductSnapshotDigest: snapshotDigest } : {}),
        },
      });
      if (result._tag === "Failure") {
        if (isAtomCommandInterrupted(result)) return;
        setActionError(
          workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
            gt("Could not record acceptance observation."),
        );
        return;
      }
      setActionNotice(
        gt("Recorded observation for {criterionId}.", { criterionId: input.criterionId }),
      );
    });
  };

  const returnTargets = validReturnTargetStages(card.stage);
  const selectedReturnTarget =
    returnStage !== null && returnTargets.includes(returnStage)
      ? returnStage
      : (returnTargets[0] ?? null);

  const handleReturnForRevision = () => {
    if (selectedReturnTarget === null) return;
    const trimmedReason = returnReason.trim();
    if (trimmedReason === "") {
      setActionError(gt("Enter a reason before returning a card for revision."));
      return;
    }
    confirmGuarded({
      title: gt("Return to {stage}?", {
        stage: mobileReturnTargetStageLabel(selectedReturnTarget, messages),
      }),
      message: gt("Downstream approvals and evidence may become stale."),
      confirmText: gt("Return for revision"),
      destructive: true,
      onConfirm: () => {
        void runGuarded("return-revision", async () => {
          const result = await returnForRevision({
            environmentId: props.environmentId,
            input: {
              cardId: card.cardId,
              expectedRevision: PositiveInt.make(card.revision),
              targetStage: selectedReturnTarget,
              reason: trimmedReason,
            },
          });
          if (result._tag === "Failure") {
            if (isAtomCommandInterrupted(result)) return;
            setActionError(
              workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
                gt("Could not return card for revision."),
            );
            return;
          }
          setReturnReason("");
          setActionNotice(
            gt("Returned card to {stage}.", {
              stage: mobileReturnTargetStageLabel(selectedReturnTarget, messages),
            }),
          );
        });
      },
    });
  };

  const blocker = workflowBoardCardBlocker(card, messages);
  const nextAction = workflowBoardCardNextAction(card, messages);
  const needsRecovery = workflowBoardNeedsRecovery(card);
  const invalidation = collectDownstreamInvalidationStates(card);
  const staleSummary = workflowBoardStaleEvidenceSummary(card);
  const unacknowledgedPolicies = workflowBoardUnacknowledgedBlockingPolicies(card);
  const threadLinks = collectMobileCardThreadLinks(card);

  const artifactErrorText =
    artifactQuery.failure !== null
      ? (workflowBoardErrorMessage(artifactQuery.failure, messages) ??
        artifactQuery.error ??
        gt("Could not load artifact."))
      : null;
  const diffErrorText =
    cardDiffQuery.failure !== null
      ? (workflowBoardErrorMessage(cardDiffQuery.failure, messages) ??
        cardDiffQuery.error ??
        gt("Could not load diff."))
      : null;

  return (
    <View className="gap-4">
      {actionError !== null ? (
        <Notice tone="danger" message={actionError} onDismiss={() => setActionError(null)} />
      ) : null}
      {actionNotice !== null ? (
        <Notice message={actionNotice} onDismiss={() => setActionNotice(null)} />
      ) : null}

      <View className="gap-2.5 rounded-2xl border border-border bg-card p-4">
        <View className="flex-row items-start justify-between gap-3">
          <View className="min-w-0 flex-1 gap-1">
            <Text textRole="title" className="font-legion-bold text-foreground">
              {card.title}
            </Text>
            <Text textRole="caption" className="text-foreground-muted">
              {workflowBoardStageLabel(card.stage, messages)} ·{" "}
              <T>
                rev <Var>{card.revision}</Var>
              </T>
            </Text>
          </View>
          <StatusPill
            label={workflowBoardCardStatusLabel(card.status, messages)}
            pillClassName={
              card.status === "blocked"
                ? "bg-danger"
                : card.status === "paused"
                  ? "bg-warning"
                  : card.status === "completed"
                    ? "bg-primary"
                    : "bg-subtle"
            }
            textClassName={
              card.status === "blocked"
                ? "text-danger-foreground"
                : card.status === "paused"
                  ? "text-warning-foreground"
                  : card.status === "completed"
                    ? "text-primary-foreground"
                    : "text-foreground-secondary"
            }
          />
        </View>
        {blocker !== null ? (
          <Text textRole="footnote" className="font-legion-medium text-danger-foreground">
            {blocker}
          </Text>
        ) : null}
        <Text textRole="footnote" className="text-foreground-secondary">
          <T>
            Next: <Var>{nextAction}</Var>
          </T>
        </Text>

        {card.status !== "completed" ? (
          <View className="flex-row flex-wrap gap-2 pt-1">
            {card.status === "paused" ? (
              <>
                <Button
                  label={gt("Resume")}
                  size="compact"
                  tone="primary"
                  icon="play"
                  disabled={!canResumeCard || busyAction !== null}
                  loading={busyAction === "resume"}
                  onPress={() => handleResume(false)}
                />
                {needsRecovery ? (
                  <Button
                    label={gt("Reconcile & resume")}
                    size="compact"
                    tone="secondary"
                    icon="arrow.clockwise"
                    disabled={!canResumeCard || busyAction !== null}
                    loading={busyAction === "reconcile-resume"}
                    onPress={() => handleResume(true)}
                  />
                ) : null}
              </>
            ) : (
              <Button
                label={gt("Pause")}
                size="compact"
                tone="secondary"
                icon="pause"
                disabled={!canPauseCard || busyAction !== null}
                loading={busyAction === "pause"}
                onPress={handlePause}
              />
            )}
          </View>
        ) : null}
      </View>

      {needsRecovery ? (
        <Notice
          tone="danger"
          icon="exclamationmark.triangle"
          message={gt(
            "Pending journal entries or external file edits must be reconciled before continuing.",
          )}
        />
      ) : null}

      {invalidation.specStale ||
      invalidation.planStale ||
      invalidation.challengeStale ||
      staleSummary.totalStale > 0 ? (
        <Notice
          icon="exclamationmark.circle"
          message={gt("Some approvals or evidence are stale and need re-validation.")}
        />
      ) : null}

      {unacknowledgedPolicies.length > 0 ? (
        <Notice
          icon="exclamationmark.triangle"
          message={gt(
            "{count} imported CLI policy item(s) require acknowledgement on desktop/web.",
            {
              count: unacknowledgedPolicies.length,
            },
          )}
        />
      ) : null}

      {threadLinks.length > 0 && props.hasPairedEnvironment ? (
        <View className="gap-2 rounded-2xl border border-border bg-card p-4">
          <Text textRole="footnote" className="font-legion-semibold text-foreground-muted">
            <T>Linked threads</T>
          </Text>
          <View className="flex-row flex-wrap gap-2">
            {threadLinks.map((link) => (
              <Button
                key={link.key}
                label={mobileCardThreadLinkLabel(link, messages)}
                size="compact"
                tone="secondary"
                icon="text.bubble"
                onPress={() => handleThreadPress(link.threadId)}
              />
            ))}
          </View>
        </View>
      ) : null}

      <SegmentedControl
        options={MOBILE_CARD_SUB_TABS.map((tab) => ({
          value: tab,
          label: mobileCardSubTabLabel(tab, messages),
        }))}
        selected={activeSubTab}
        onSelect={(tab) => setSelectedSubTab(tab)}
      />

      {activeSubTab === "spec" ? (
        <SpecInspectSection
          card={card}
          selectedArtifactKey={selectedArtifactKey}
          onSelectArtifactKey={setSelectedArtifactKey}
          artifact={artifactQuery.data}
          artifactLoading={artifactQuery.isPending}
          artifactError={artifactErrorText}
          canApproveSpec={canApproveSpec}
          busyAction={busyAction}
          onApproveSpec={handleApproveSpec}
        />
      ) : activeSubTab === "plan" ? (
        <PlanInspectSection
          card={card}
          canApprovePlan={canApprovePlan}
          busyAction={busyAction}
          onApprovePlan={handleApprovePlan}
        />
      ) : activeSubTab === "execute" ? (
        <ExecuteInspectSection
          card={card}
          cardDiff={cardDiffQuery.data}
          cardDiffLoading={cardDiffQuery.isPending}
          cardDiffError={diffErrorText}
          onRefreshDiff={() => cardDiffQuery.refresh()}
          canRetryStep={canRetryStep}
          canResolveFinding={canResolveFinding}
          canOpenThread={props.hasPairedEnvironment}
          busyAction={busyAction}
          onOpenThread={handleThreadPress}
          onRetryStep={handleRetryStep}
          onResolveFinding={handleResolveFinding}
        />
      ) : activeSubTab === "ship" ? (
        <ShipInspectSection
          card={card}
          cardDiff={cardDiffQuery.data}
          cardDiffLoading={cardDiffQuery.isPending}
          cardDiffError={diffErrorText}
          onRefreshDiff={() => cardDiffQuery.refresh()}
          canRecordAcceptance={canRecordAcceptance}
          canApproveDelivery={canApproveDelivery}
          busyAction={busyAction}
          onRecordAcceptance={handleRecordAcceptance}
          onApproveDelivery={handleApproveDelivery}
          onNavigateToFix={(navTarget) => setSelectedSubTab(navTarget.subTab)}
        />
      ) : (
        <HistoryInspectSection card={card} />
      )}

      {returnTargets.length > 0 && card.status !== "completed" && canReturn ? (
        <View className="gap-2.5 rounded-2xl border border-border bg-card p-4">
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>Return for revision</T>
          </Text>
          <View className="flex-row flex-wrap gap-1.5">
            {returnTargets.map((stage) => (
              <Chip
                key={stage}
                size="small"
                label={mobileReturnTargetStageLabel(stage, messages)}
                selected={selectedReturnTarget === stage}
                onPress={() => setReturnStage(stage)}
              />
            ))}
          </View>
          <AppTextInput
            accessibilityLabel={gt("Reason for returning card")}
            placeholder={gt("Reason for revision (required)")}
            value={returnReason}
            onChangeText={setReturnReason}
          />
          <Button
            label={gt("Return for revision")}
            tone="danger"
            icon="arrow.uturn.backward"
            disabled={busyAction !== null}
            loading={busyAction === "return-revision"}
            onPress={handleReturnForRevision}
          />
        </View>
      ) : null}
    </View>
  );
}

export function WorkflowCardRouteScreen({ route }: StaticScreenProps<WorkflowCardRouteParams>) {
  const gt = useGT();
  const messages = useMessages();
  const navigation = useNavigation();
  const insets = useSafeAreaInsets();
  const { isReady, environments } = useEnvironments();
  const projects = useProjects();

  const rawEnvId = route?.params?.environmentId;
  const rawProjectId = route?.params?.projectId;
  const rawCardId = route?.params?.cardId;
  const stage = route?.params?.stage;

  const typedEnvironmentId = Option.getOrNull(decodeEnvironmentId(rawEnvId));
  const typedProjectId = Option.getOrNull(decodeProjectId(rawProjectId));
  const typedCardId = Option.getOrNull(decodeWorkflowCardId(rawCardId));
  const hasValidParams =
    typedEnvironmentId !== null && typedProjectId !== null && typedCardId !== null;

  const target = resolveMobileWorkflowBoardTarget({
    isReady,
    environments,
    projects,
    requestedEnvironmentId: typedEnvironmentId ?? rawEnvId,
    requestedProjectId: typedProjectId ?? rawProjectId,
  });

  const canReadBoard = useAtomValue(workflowBoard.readPermissionAtom(typedEnvironmentId));
  const boardEnabled =
    hasValidParams &&
    !target.loading &&
    !target.missingRequestedProject &&
    target.availability === "enabled";

  const cardQuery = useEnvironmentQuery(
    boardEnabled &&
      canReadBoard &&
      typedEnvironmentId !== null &&
      typedProjectId !== null &&
      typedCardId !== null
      ? workflowBoard.cardDetail({
          environmentId: typedEnvironmentId,
          input: { cardId: typedCardId, projectId: typedProjectId },
        })
      : null,
  );

  const loadingNotice = workflowBoardAvailabilityNotice("loading", messages);
  const availabilityNotice = workflowBoardAvailabilityNotice(target.availability, messages);
  const readPermissionNotice = workflowBoardReadPermissionNotice(messages);
  const projectNotFoundNotice = workflowBoardProjectNotFoundNotice(messages);
  const cardErrorText =
    cardQuery.failure !== null
      ? (workflowBoardErrorMessage(cardQuery.failure, messages) ??
        cardQuery.error ??
        gt("Could not load workflow card."))
      : null;

  const hasPairedEnvironment =
    typedEnvironmentId !== null &&
    environments.some((env) => env.environmentId === typedEnvironmentId);

  return (
    <View className="flex-1 bg-screen">
      <AppBar
        title={cardQuery.data?.title ?? gt("Workflow card")}
        subtitle={
          hasValidParams && target.selected
            ? `${target.selected.projectTitle} · ${target.selected.environmentLabel}`
            : null
        }
        onBack={navigation.canGoBack() ? () => navigation.goBack() : undefined}
        refresh={
          boardEnabled && canReadBoard
            ? {
                accessibilityLabel: gt("Refresh workflow card"),
                disabled: cardQuery.isPending,
                onPress: () => cardQuery.refresh(),
              }
            : undefined
        }
      />
      <ScreenContent surface="screen">
        <ScrollView
          className="flex-1"
          contentInsetAdjustmentBehavior="automatic"
          showsVerticalScrollIndicator={false}
          contentContainerClassName="gap-4 px-5 pt-4"
          contentContainerStyle={{ paddingBottom: Math.max(insets.bottom, 20) + 16 }}
        >
          {!hasValidParams ? (
            <EmptyState
              title={projectNotFoundNotice.title}
              detail={projectNotFoundNotice.description}
            />
          ) : target.loading || target.availability === "loading" ? (
            <EmptyState
              title={loadingNotice?.title ?? gt("Loading workflow board…")}
              detail={
                loadingNotice?.description ?? gt("Checking server workflow board capabilities.")
              }
            />
          ) : target.missingRequestedProject ? (
            <EmptyState
              title={projectNotFoundNotice.title}
              detail={projectNotFoundNotice.description}
            />
          ) : availabilityNotice !== null ? (
            <EmptyState
              title={availabilityNotice.title}
              detail={availabilityNotice.description}
              actionLabel={
                availabilityNotice.action === "open-labs-settings"
                  ? gt("Open Labs settings")
                  : undefined
              }
              onAction={
                availabilityNotice.action === "open-labs-settings"
                  ? () => openSettings(navigation, "SettingsLabs")
                  : undefined
              }
            />
          ) : !canReadBoard ? (
            <EmptyState
              title={readPermissionNotice.title}
              detail={readPermissionNotice.description}
            />
          ) : cardErrorText !== null && cardQuery.data === null ? (
            <View className="gap-3">
              <Notice tone="danger" message={cardErrorText} />
              <Button label={gt("Retry")} tone="secondary" onPress={() => cardQuery.refresh()} />
            </View>
          ) : cardQuery.data === null ? (
            <EmptyState
              title={gt("Loading workflow card…")}
              detail={gt("Fetching card artifacts, tasks, checks, and evidence.")}
            />
          ) : (
            <>
              {cardErrorText !== null ? <Notice tone="danger" message={cardErrorText} /> : null}
              <WorkflowCardBody
                key={workflowCardStateKey(typedEnvironmentId, typedProjectId, typedCardId)}
                environmentId={typedEnvironmentId}
                projectId={typedProjectId}
                card={cardQuery.data}
                requestedStage={stage}
                hasPairedEnvironment={hasPairedEnvironment}
              />
            </>
          )}
        </ScrollView>
      </ScreenContent>
    </View>
  );
}

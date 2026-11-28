import type {
  WorkflowBoardArtifact,
  WorkflowBoardFindingResolveDisposition,
  WorkflowBoardGetCardDiffResult,
  WorkflowBoardManualAcceptanceVerdict,
  WorkflowCardDetail,
} from "@legioncode/contracts";
import { T, useGT, useMessages, Var } from "gt-react-native";
import { useState } from "react";
import { View } from "react-native";

import { AppText as Text, AppTextInput } from "../../components/ui/AppText";
import { Button } from "../../components/ui/Button";
import { Chip } from "../../components/ui/Chip";
import { ListSection } from "../../components/ui/ListSection";
import { Notice } from "../../components/ui/Notice";
import {
  WORKFLOW_BOARD_PLAN_LENSES,
  canApproveWorkflowBoardDelivery,
  canApproveWorkflowBoardPlan,
  canApproveWorkflowBoardSpec,
  collectPlanApprovalBlockers,
  isAcObservationStale,
  isCheckReceiptStale,
  isReviewReportStale,
  isWorkflowBoardCardShipped,
  mobileFindingDispositionLabel,
  mobileManualAcceptanceVerdictLabel,
  mobilePlanApprovalBlockerMessage,
  workflowBoardAcEvidenceStatusLabel,
  workflowBoardCheckStatusLabel,
  workflowBoardCliImportHistoricalReceiptKindLabel,
  workflowBoardCliImportPolicyStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardPlanApprovalLabel,
  workflowBoardRecoverySummary,
  workflowBoardReviewVerdictLabel,
  workflowBoardSpecApprovalLabel,
  workflowBoardStageLabel,
  workflowBoardTaskStatusMap,
  workflowBoardUnresolvedFindings,
  type WorkflowBoardFixNavigation,
  resolveFixPointerNavigation,
} from "./workflowBoard.logic";

function SectionCard(props: { readonly children: React.ReactNode }) {
  return (
    <View className="gap-3 rounded-2xl border border-border bg-card p-4">{props.children}</View>
  );
}

export function SpecInspectSection(props: {
  readonly card: WorkflowCardDetail;
  readonly selectedArtifactKey: string | null;
  readonly onSelectArtifactKey: (key: string | null) => void;
  readonly artifact: WorkflowBoardArtifact | null;
  readonly artifactLoading: boolean;
  readonly artifactError: string | null;
  readonly canApproveSpec: boolean;
  readonly busyAction: string | null;
  readonly onApproveSpec: () => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;
  const canApprove = canApproveWorkflowBoardSpec({
    detail: card,
    canOperate: props.canApproveSpec,
  });
  const unresolvedCount = card.exploration?.unresolvedIds.length ?? 0;
  const challengeConcerns = card.challengeReceipt?.concerns ?? [];

  return (
    <View className="gap-4">
      <SectionCard>
        <View className="flex-row items-center justify-between gap-2">
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>Spec summary</T>
          </Text>
          <Text textRole="caption" className="text-foreground-muted">
            {workflowBoardSpecApprovalLabel(card.specApprovalStatus, messages)}
          </Text>
        </View>
        <Text textRole="caption" className="text-foreground-muted">
          <T>
            Source: <Var>{card.spec.sourceType}</Var> · Assistance:{" "}
            <Var>{card.spec.assistanceMode}</Var>
          </T>
        </Text>
        <Text textRole="body" className="leading-relaxed text-foreground" selectable>
          {card.spec.brief.trim() !== "" ? card.spec.brief : gt("No spec brief written yet.")}
        </Text>
        {card.specApproval ? (
          <Text textRole="caption" className="text-foreground-muted">
            <T>
              Approved rev <Var>{card.specApproval.revision}</Var> by{" "}
              <Var>{card.specApproval.approvedBy}</Var> at <Var>{card.specApproval.approvedAt}</Var>
            </T>
          </Text>
        ) : null}
        {card.stage === "spec" || card.specApprovalStatus !== "valid" ? (
          <Button
            label={
              props.busyAction === "approve-spec"
                ? gt("Approving…")
                : card.specApprovalStatus === "valid"
                  ? gt("Spec approved")
                  : gt("Approve spec")
            }
            tone="primary"
            icon="checkmark.circle"
            disabled={!canApprove || props.busyAction !== null}
            loading={props.busyAction === "approve-spec"}
            onPress={props.onApproveSpec}
          />
        ) : null}
      </SectionCard>

      <SectionCard>
        <Text textRole="headline" className="font-legion-semibold text-foreground">
          {gt("Acceptance criteria ({count})", { count: card.spec.acceptanceCriteria.length })}
        </Text>
        {card.spec.acceptanceCriteria.length === 0 ? (
          <Text textRole="footnote" className="text-foreground-muted">
            <T>No acceptance criteria recorded.</T>
          </Text>
        ) : (
          <View className="gap-2">
            {card.spec.acceptanceCriteria.map((criterion) => (
              <View key={criterion.id} className="rounded-xl bg-subtle p-3">
                <Text textRole="caption" className="font-legion-semibold text-foreground-muted">
                  {criterion.id}
                </Text>
                <Text textRole="footnote" className="mt-1 text-foreground">
                  {criterion.text}
                </Text>
              </View>
            ))}
          </View>
        )}
        {card.spec.constraints.length > 0 ? (
          <View className="gap-1.5 pt-1">
            <Text textRole="footnote" className="font-legion-semibold text-foreground-muted">
              <T>Constraints</T>
            </Text>
            {card.spec.constraints.map((constraint) => (
              <Text key={constraint} textRole="footnote" className="text-foreground">
                • {constraint}
              </Text>
            ))}
          </View>
        ) : null}
        {card.spec.sourceRefs.length > 0 ? (
          <View className="gap-1.5 pt-1">
            <Text textRole="footnote" className="font-legion-semibold text-foreground-muted">
              <T>Source references</T>
            </Text>
            {card.spec.sourceRefs.map((ref) => (
              <Text key={ref} textRole="caption" className="text-foreground-secondary" selectable>
                {ref}
              </Text>
            ))}
          </View>
        ) : null}
      </SectionCard>

      {card.exploration !== undefined && card.exploration !== null ? (
        <SectionCard>
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>Guided exploration</T>
          </Text>
          <Text textRole="caption" className="text-foreground-muted">
            <T>
              Round <Var>{card.exploration.roundIndex}</Var> · Unresolved:{" "}
              <Var>{unresolvedCount}</Var>
            </T>
            {card.exploration.paused ? ` · ${gt("Paused")}` : ""}
          </Text>
          {card.exploration.decisions.map((decision) => (
            <View key={decision.id} className="gap-1 rounded-xl bg-subtle p-3">
              <Text textRole="caption" className="font-legion-semibold text-foreground-muted">
                {decision.id} ({decision.status})
              </Text>
              <Text textRole="footnote" className="text-foreground">
                {decision.prompt}
              </Text>
              {decision.answer ? (
                <Text textRole="footnote" className="text-foreground-secondary">
                  <T>
                    Answer: <Var>{decision.answer}</Var>
                  </T>
                </Text>
              ) : null}
            </View>
          ))}
        </SectionCard>
      ) : null}

      {card.challengeReceipt !== undefined && card.challengeReceipt !== null ? (
        <SectionCard>
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>Bounded challenge</T>
          </Text>
          <Text textRole="caption" className="text-foreground-muted">
            <T>
              Status: <Var>{card.challengeReceipt.status}</Var> · Concerns:{" "}
              <Var>{challengeConcerns.length}</Var>
            </T>
          </Text>
          {challengeConcerns.map((concern) => (
            <View key={concern.id} className="gap-1 rounded-xl bg-subtle p-3">
              <Text textRole="caption" className="font-legion-semibold text-foreground-muted">
                {concern.id} ({concern.disposition})
              </Text>
              <Text textRole="footnote" className="text-foreground">
                {concern.summary}
              </Text>
              {concern.answer ? (
                <Text textRole="caption" className="text-foreground-secondary">
                  <T>
                    Answer: <Var>{concern.answer}</Var>
                  </T>
                </Text>
              ) : null}
              {concern.reason ? (
                <Text textRole="caption" className="text-foreground-secondary">
                  <T>
                    Reason: <Var>{concern.reason}</Var>
                  </T>
                </Text>
              ) : null}
            </View>
          ))}
        </SectionCard>
      ) : null}

      {card.artifactKeys.length > 0 ? (
        <SectionCard>
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>Artifacts</T>
          </Text>
          <View className="flex-row flex-wrap gap-2">
            {card.artifactKeys.map((key) => (
              <Chip
                key={key}
                label={key}
                size="small"
                selected={props.selectedArtifactKey === key}
                onPress={() =>
                  props.onSelectArtifactKey(props.selectedArtifactKey === key ? null : key)
                }
              />
            ))}
          </View>
          {props.selectedArtifactKey !== null ? (
            props.artifactError !== null ? (
              <Notice tone="danger" message={props.artifactError} />
            ) : props.artifactLoading && props.artifact === null ? (
              <Text textRole="footnote" className="text-foreground-muted">
                <T>Loading artifact…</T>
              </Text>
            ) : props.artifact !== null ? (
              <View className="gap-1.5 rounded-xl bg-subtle p-3">
                <Text textRole="caption" className="font-legion-medium text-foreground-muted">
                  {props.artifact.relativePath}
                </Text>
                <Text textRole="caption" className="text-foreground" selectable>
                  {props.artifact.content}
                </Text>
              </View>
            ) : null
          ) : null}
        </SectionCard>
      ) : null}
    </View>
  );
}

export function PlanInspectSection(props: {
  readonly card: WorkflowCardDetail;
  readonly canApprovePlan: boolean;
  readonly busyAction: string | null;
  readonly onApprovePlan: () => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;
  const plan = card.plan ?? null;
  const lenses = plan?.lenses ?? card.lenses ?? [];
  const alternatives = plan?.alternatives ?? card.alternatives ?? [];
  const selectedAlternativeId = plan?.selectedAlternativeId ?? card.selectedAlternativeId ?? null;
  const selectionRationale = plan?.selectionRationale ?? card.selectionRationale ?? null;
  const strategy = plan?.strategy ?? card.strategy ?? null;
  const strategyRationale = plan?.strategyRationale ?? card.strategyRationale ?? null;
  const granularity = plan?.granularity ?? card.granularity ?? null;
  const tasks = plan?.tasks ?? card.tasks ?? [];
  const acMappings = plan?.acMappings ?? card.acMappings ?? [];
  const taskStatuses = workflowBoardTaskStatusMap(card);
  const blockers = collectPlanApprovalBlockers(card);
  const canApprove = canApproveWorkflowBoardPlan({
    detail: card,
    canOperate: props.canApprovePlan,
  });

  return (
    <View className="gap-4">
      <SectionCard>
        <View className="flex-row items-center justify-between gap-2">
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>Plan summary</T>
          </Text>
          <Text textRole="caption" className="text-foreground-muted">
            {workflowBoardPlanApprovalLabel(card.planApprovalStatus, messages)}
          </Text>
        </View>
        <Text textRole="footnote" className="text-foreground-secondary">
          <T>
            Strategy: <Var>{strategy ?? gt("Unset")}</Var> · Granularity:{" "}
            <Var>{granularity ?? gt("Unset")}</Var>
          </T>
        </Text>
        {strategyRationale ? (
          <Text textRole="footnote" className="text-foreground-muted">
            {strategyRationale}
          </Text>
        ) : null}
        {selectedAlternativeId ? (
          <Text textRole="footnote" className="text-foreground">
            <T>
              Selected alternative: <Var>{selectedAlternativeId}</Var>
            </T>
            {selectionRationale ? ` (${selectionRationale})` : ""}
          </Text>
        ) : null}
        {card.planApproval ? (
          <Text textRole="caption" className="text-foreground-muted">
            <T>
              Approved rev <Var>{card.planApproval.revision}</Var> by{" "}
              <Var>{card.planApproval.approvedBy}</Var>
            </T>
          </Text>
        ) : null}
        {blockers.length > 0 && card.planApprovalStatus !== "valid" ? (
          <View className="gap-1 rounded-xl bg-subtle p-3">
            <Text textRole="caption" className="font-legion-semibold text-foreground">
              <T>Remaining Plan approval gates</T>
            </Text>
            {blockers.map((code) => (
              <Text key={code} textRole="caption" className="text-foreground-muted">
                • {mobilePlanApprovalBlockerMessage(code, messages)}
              </Text>
            ))}
          </View>
        ) : null}
        {card.stage === "plan" || card.planApprovalStatus !== "valid" ? (
          <Button
            label={
              props.busyAction === "approve-plan"
                ? gt("Approving…")
                : card.planApprovalStatus === "valid"
                  ? gt("Plan approved")
                  : gt("Approve plan")
            }
            tone="primary"
            icon="checkmark.circle"
            disabled={!canApprove || props.busyAction !== null}
            loading={props.busyAction === "approve-plan"}
            onPress={props.onApprovePlan}
          />
        ) : null}
      </SectionCard>

      <SectionCard>
        <Text textRole="headline" className="font-legion-semibold text-foreground">
          <T>Design lenses</T>
        </Text>
        {WORKFLOW_BOARD_PLAN_LENSES.map((lens) => {
          const entry = lenses.find((candidate) => candidate.lens === lens);
          return (
            <View key={lens} className="gap-1 rounded-xl bg-subtle p-3">
              <Text textRole="caption" className="font-legion-semibold text-foreground">
                {lens}:{" "}
                {entry?.applicable === true
                  ? gt("Applicable")
                  : entry?.applicable === false
                    ? gt("Not applicable")
                    : gt("Undecided")}
              </Text>
              {entry?.summary ? (
                <Text textRole="caption" className="text-foreground-secondary">
                  {entry.summary}
                </Text>
              ) : entry?.reason ? (
                <Text textRole="caption" className="text-foreground-muted">
                  {entry.reason}
                </Text>
              ) : null}
            </View>
          );
        })}
      </SectionCard>

      {alternatives.length > 0 ? (
        <SectionCard>
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>Compared alternatives</T>
          </Text>
          {alternatives.map((alt) => (
            <View key={alt.id} className="gap-1 rounded-xl bg-subtle p-3">
              <Text textRole="footnote" className="font-legion-semibold text-foreground">
                {alt.title} ({alt.id})
                {selectedAlternativeId === alt.id ? ` · ${gt("Selected")}` : ""}
              </Text>
              <Text textRole="caption" className="text-foreground-secondary">
                {alt.summary}
              </Text>
              <Text textRole="caption" className="text-foreground-muted">
                <T>
                  Tradeoffs: <Var>{alt.tradeoffs}</Var>
                </T>
              </Text>
            </View>
          ))}
        </SectionCard>
      ) : null}

      <SectionCard>
        <Text textRole="headline" className="font-legion-semibold text-foreground">
          {gt("Task contracts ({count})", { count: tasks.length })}
        </Text>
        {tasks.length === 0 ? (
          <Text textRole="footnote" className="text-foreground-muted">
            <T>No tasks defined yet.</T>
          </Text>
        ) : (
          tasks.map((task) => {
            const status = taskStatuses.get(task.taskId) ?? task.status ?? "todo";
            return (
              <View key={task.taskId} className="gap-1 rounded-xl bg-subtle p-3">
                <View className="flex-row items-center justify-between gap-2">
                  <Text textRole="footnote" className="font-legion-semibold text-foreground">
                    {task.taskId}: {task.title}
                  </Text>
                  <Text textRole="caption" className="text-foreground-muted">
                    {status}
                  </Text>
                </View>
                <Text textRole="caption" className="text-foreground-secondary" selectable>
                  <T>
                    Allowed: <Var>{task.filesAllowed.join(", ")}</Var>
                  </T>
                </Text>
                {(task.dependsOn?.length ?? 0) > 0 ? (
                  <Text textRole="caption" className="text-foreground-muted">
                    <T>
                      Depends on: <Var>{task.dependsOn?.join(", ") ?? ""}</Var>
                    </T>
                  </Text>
                ) : null}
              </View>
            );
          })
        )}
      </SectionCard>

      <SectionCard>
        <Text textRole="headline" className="font-legion-semibold text-foreground">
          {gt("Criterion evidence mappings ({count})", { count: acMappings.length })}
        </Text>
        {acMappings.length === 0 ? (
          <Text textRole="footnote" className="text-foreground-muted">
            <T>No criterion evidence mappings yet.</T>
          </Text>
        ) : (
          acMappings.map((mapping) => (
            <View key={mapping.criterionId} className="gap-1 rounded-xl bg-subtle p-3">
              <Text textRole="caption" className="font-legion-semibold text-foreground">
                {mapping.criterionId} · {mapping.method}
              </Text>
              {mapping.checkIds && mapping.checkIds.length > 0 ? (
                <Text textRole="caption" className="text-foreground-secondary">
                  <T>
                    Checks: <Var>{mapping.checkIds.join(", ")}</Var>
                  </T>
                </Text>
              ) : null}
              {mapping.procedure ? (
                <Text textRole="caption" className="text-foreground-secondary">
                  {mapping.procedure}
                </Text>
              ) : null}
              {mapping.externalRef ? (
                <Text textRole="caption" className="text-foreground-muted">
                  {mapping.externalRef}
                </Text>
              ) : null}
            </View>
          ))
        )}
      </SectionCard>
    </View>
  );
}

export function DiffSummaryCard(props: {
  readonly cardDiff: WorkflowBoardGetCardDiffResult | null;
  readonly loading: boolean;
  readonly error: string | null;
  readonly onRefresh: () => void;
}) {
  const gt = useGT();
  const [expandedFile, setExpandedFile] = useState<string | null>(null);

  return (
    <SectionCard>
      <View className="flex-row items-center justify-between gap-2">
        <Text textRole="headline" className="font-legion-semibold text-foreground">
          <T>Worktree diff</T>
        </Text>
        <Button label={gt("Refresh diff")} size="compact" tone="text" onPress={props.onRefresh} />
      </View>
      {props.error !== null ? <Notice tone="danger" message={props.error} /> : null}
      {props.loading && props.cardDiff === null ? (
        <Text textRole="footnote" className="text-foreground-muted">
          <T>Loading diff…</T>
        </Text>
      ) : props.cardDiff === null ? (
        <Text textRole="footnote" className="text-foreground-muted">
          <T>No diff recorded yet.</T>
        </Text>
      ) : (
        <View className="gap-2">
          {props.cardDiff.worktreePath ? (
            <Text textRole="caption" className="text-foreground-muted" selectable>
              {props.cardDiff.worktreePath}
            </Text>
          ) : null}
          {props.cardDiff.files.length === 0 ? (
            <Text textRole="footnote" className="text-foreground-muted">
              <T>Working tree is clean.</T>
            </Text>
          ) : (
            props.cardDiff.files.map((file) => {
              const isOpen = expandedFile === file.path;
              return (
                <View key={file.path} className="gap-1.5 rounded-xl bg-subtle p-3">
                  <View className="flex-row items-center justify-between gap-2">
                    <Text
                      textRole="caption"
                      className="min-w-0 flex-1 font-legion-medium text-foreground"
                      selectable
                    >
                      {file.path}
                    </Text>
                    <Text textRole="caption" className="text-foreground-muted">
                      +{file.additions} / -{file.deletions}
                    </Text>
                  </View>
                  <Text textRole="caption" className="text-foreground-muted">
                    {file.changeType}
                    {file.contractStatus ? ` · ${file.contractStatus}` : ""}
                  </Text>
                  {file.patch ? (
                    <Button
                      label={isOpen ? gt("Hide patch") : gt("Show patch")}
                      size="compact"
                      tone="text"
                      onPress={() => setExpandedFile(isOpen ? null : file.path)}
                    />
                  ) : null}
                  {isOpen && file.patch ? (
                    <Text textRole="caption" className="text-foreground-secondary" selectable>
                      {file.patch}
                    </Text>
                  ) : null}
                </View>
              );
            })
          )}
        </View>
      )}
    </SectionCard>
  );
}

const DISPOSITIONS: readonly WorkflowBoardFindingResolveDisposition[] = [
  "fix",
  "accept_risk",
  "dismiss",
  "defer",
];

export function ExecuteInspectSection(props: {
  readonly card: WorkflowCardDetail;
  readonly cardDiff: WorkflowBoardGetCardDiffResult | null;
  readonly cardDiffLoading: boolean;
  readonly cardDiffError: string | null;
  readonly onRefreshDiff: () => void;
  readonly canRetryStep: boolean;
  readonly canResolveFinding: boolean;
  readonly canOpenThread: boolean;
  readonly busyAction: string | null;
  readonly onOpenThread: (threadId: WorkflowCardDetail["activeThreadId"]) => void;
  readonly onRetryStep: (
    stepKind: "task" | "check" | "review" | "stage",
    targetId?: string,
  ) => void;
  readonly onResolveFinding: (input: {
    readonly findingId: string;
    readonly disposition: WorkflowBoardFindingResolveDisposition;
    readonly reason: string;
  }) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;
  const recovery = workflowBoardRecoverySummary(card);
  const currentSnapshotDigest =
    card.shipReadiness?.productSnapshotDigest ?? props.cardDiff?.productSnapshotDigest ?? null;
  const taskExecutions = card.taskExecutions ?? [];
  const contractReports = card.fileContractReports ?? [];
  const checkReceipts = card.checkReceipts ?? [];
  const reviewReport = card.reviewReport ?? null;
  const unresolvedFindings = workflowBoardUnresolvedFindings(card);

  const [findingDrafts, setFindingDrafts] = useState<
    Readonly<
      Record<
        string,
        { readonly disposition: WorkflowBoardFindingResolveDisposition; readonly reason: string }
      >
    >
  >({});

  const draftFor = (findingId: string) =>
    findingDrafts[findingId] ?? { disposition: "fix" as const, reason: "" };

  return (
    <View className="gap-4">
      {recovery.requiresReconciliation ? (
        <SectionCard>
          <Text textRole="headline" className="font-legion-semibold text-warning-foreground">
            <T>Recovery and external edit reconciliation</T>
          </Text>
          <Text textRole="footnote" className="text-foreground">
            <T>
              Pending journal entries: <Var>{recovery.incompleteJournalCount}</Var> · Detected
              external edits: <Var>{recovery.detectedExternalEdits.length}</Var>
            </T>
          </Text>
          {recovery.detectedExternalEdits.map((edit) => (
            <View key={edit.id} className="rounded-xl bg-subtle p-3">
              <Text textRole="caption" className="font-legion-medium text-foreground" selectable>
                {edit.relativePath}
              </Text>
              <Text textRole="caption" className="text-foreground-muted">
                {edit.summary}
              </Text>
            </View>
          ))}
        </SectionCard>
      ) : null}

      <SectionCard>
        <View className="flex-row items-center justify-between gap-2">
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            {gt("Task executions ({count})", { count: taskExecutions.length })}
          </Text>
          {props.canRetryStep &&
          card.stage === "execute" &&
          card.status !== "paused" &&
          card.status !== "completed" ? (
            <Button
              label={gt("Retry stage")}
              size="compact"
              tone="secondary"
              disabled={props.busyAction !== null}
              loading={props.busyAction === "retry:stage"}
              onPress={() => props.onRetryStep("stage")}
            />
          ) : null}
        </View>
        {taskExecutions.length === 0 ? (
          <Text textRole="footnote" className="text-foreground-muted">
            <T>No task executions started yet.</T>
          </Text>
        ) : (
          taskExecutions.map((exec) => (
            <View key={exec.taskId} className="gap-2 rounded-xl bg-subtle p-3">
              <View className="flex-row items-center justify-between gap-2">
                <Text textRole="footnote" className="font-legion-semibold text-foreground">
                  {exec.taskId} ·{" "}
                  <T>
                    Attempt <Var>{exec.attempt}</Var>
                  </T>
                </Text>
                <Text textRole="caption" className="text-foreground-muted">
                  {exec.status}
                </Text>
              </View>
              {exec.lastError ? (
                <Text textRole="caption" className="text-danger-foreground">
                  {exec.lastError}
                </Text>
              ) : null}
              <View className="flex-row flex-wrap gap-2">
                {exec.threadId !== null && props.canOpenThread ? (
                  <Button
                    label={gt("Open thread")}
                    size="compact"
                    tone="secondary"
                    icon="text.bubble"
                    onPress={() => props.onOpenThread(exec.threadId)}
                  />
                ) : null}
                {props.canRetryStep &&
                (exec.status === "failed" || exec.status === "blocked") &&
                card.status !== "paused" &&
                card.status !== "completed" ? (
                  <Button
                    label={gt("Retry task")}
                    size="compact"
                    tone="secondary"
                    icon="arrow.clockwise"
                    disabled={props.busyAction !== null}
                    loading={props.busyAction === `retry:task:${exec.taskId}`}
                    onPress={() => props.onRetryStep("task", exec.taskId)}
                  />
                ) : null}
              </View>
            </View>
          ))
        )}
      </SectionCard>

      {contractReports.length > 0 ? (
        <SectionCard>
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>File contract validation</T>
          </Text>
          {contractReports.map((report) => (
            <View
              key={`${report.taskId}:${report.updatedAt}`}
              className="gap-1 rounded-xl bg-subtle p-3"
            >
              <Text textRole="footnote" className="font-legion-semibold text-foreground">
                {report.taskId} · {report.status}
              </Text>
              {report.blockingReason ? (
                <Text textRole="caption" className="text-danger-foreground">
                  {report.blockingReason}
                </Text>
              ) : null}
              {report.unexpectedFiles.length > 0 ? (
                <Text textRole="caption" className="text-warning-foreground" selectable>
                  <T>
                    Unexpected: <Var>{report.unexpectedFiles.join(", ")}</Var>
                  </T>
                </Text>
              ) : null}
              {report.forbiddenFiles.length > 0 ? (
                <Text textRole="caption" className="text-danger-foreground" selectable>
                  <T>
                    Forbidden: <Var>{report.forbiddenFiles.join(", ")}</Var>
                  </T>
                </Text>
              ) : null}
            </View>
          ))}
        </SectionCard>
      ) : null}

      <SectionCard>
        <Text textRole="headline" className="font-legion-semibold text-foreground">
          {gt("Planned checks ({count})", { count: checkReceipts.length })}
        </Text>
        {checkReceipts.length === 0 ? (
          <Text textRole="footnote" className="text-foreground-muted">
            <T>No planned checks recorded yet.</T>
          </Text>
        ) : (
          checkReceipts.map((receipt) => {
            const stale = isCheckReceiptStale(receipt, currentSnapshotDigest);
            return (
              <View key={receipt.checkId} className="gap-2 rounded-xl bg-subtle p-3">
                <View className="flex-row items-center justify-between gap-2">
                  <Text textRole="footnote" className="font-legion-semibold text-foreground">
                    {receipt.checkId}
                  </Text>
                  <Text
                    textRole="caption"
                    className={
                      receipt.status === "failed"
                        ? "font-legion-medium text-danger-foreground"
                        : stale
                          ? "font-legion-medium text-warning-foreground"
                          : "text-foreground-muted"
                    }
                  >
                    {workflowBoardCheckStatusLabel(receipt.status, messages)}
                    {stale && receipt.status !== "stale" ? ` (${gt("Stale")})` : ""}
                  </Text>
                </View>
                <Text textRole="caption" className="text-foreground-secondary" selectable>
                  {receipt.argv.join(" ")}
                </Text>
                {receipt.stderrExcerpt ? (
                  <Text textRole="caption" className="text-danger-foreground" selectable>
                    {receipt.stderrExcerpt}
                  </Text>
                ) : null}
                {props.canRetryStep &&
                card.status !== "paused" &&
                card.status !== "completed" &&
                (receipt.status === "failed" ||
                  receipt.status === "stale" ||
                  receipt.status === "pending" ||
                  stale) ? (
                  <Button
                    label={receipt.status === "pending" ? gt("Run check") : gt("Retry check")}
                    size="compact"
                    tone="secondary"
                    icon="arrow.clockwise"
                    disabled={props.busyAction !== null}
                    loading={props.busyAction === `retry:check:${receipt.checkId}`}
                    onPress={() => props.onRetryStep("check", receipt.checkId)}
                  />
                ) : null}
              </View>
            );
          })
        )}
      </SectionCard>

      <SectionCard>
        <Text textRole="headline" className="font-legion-semibold text-foreground">
          <T>Independent review</T>
        </Text>
        {reviewReport === null ? (
          <Text textRole="footnote" className="text-foreground-muted">
            <T>No independent review recorded yet.</T>
          </Text>
        ) : (
          <View className="gap-2.5">
            <View className="flex-row items-center justify-between gap-2">
              <Text textRole="footnote" className="font-legion-semibold text-foreground">
                {workflowBoardReviewVerdictLabel(reviewReport.verdict, messages)}
              </Text>
              <Text textRole="caption" className="text-foreground-muted">
                {reviewReport.status}
                {isReviewReportStale(reviewReport, currentSnapshotDigest)
                  ? ` · ${gt("Stale")}`
                  : ""}
              </Text>
            </View>
            {reviewReport.summary ? (
              <Text textRole="footnote" className="text-foreground-secondary">
                {reviewReport.summary}
              </Text>
            ) : null}
            <View className="flex-row flex-wrap gap-2">
              {reviewReport.threadId !== null && props.canOpenThread ? (
                <Button
                  label={gt("Open Review thread")}
                  size="compact"
                  tone="secondary"
                  icon="text.bubble"
                  onPress={() => props.onOpenThread(reviewReport.threadId)}
                />
              ) : null}
              {props.canRetryStep &&
              (reviewReport.status === "failed" || reviewReport.verdict === "failed") &&
              card.status !== "paused" &&
              card.status !== "completed" ? (
                <Button
                  label={gt("Retry review")}
                  size="compact"
                  tone="secondary"
                  icon="arrow.clockwise"
                  disabled={props.busyAction !== null}
                  loading={props.busyAction === "retry:review"}
                  onPress={() => props.onRetryStep("review")}
                />
              ) : null}
            </View>
            {reviewReport.findings.length > 0 ? (
              <Text textRole="footnote" className="font-legion-medium text-foreground">
                {gt("Findings ({open} unresolved / {total} total)", {
                  open: unresolvedFindings.length,
                  total: reviewReport.findings.length,
                })}
              </Text>
            ) : null}
            {reviewReport.findings.map((finding) => {
              const draft = draftFor(finding.id);
              const isOpen =
                finding.status === "open" ||
                (finding.status === undefined && finding.disposition === "unresolved");
              return (
                <View key={finding.id} className="gap-2 rounded-xl bg-subtle p-3">
                  <Text textRole="caption" className="font-legion-semibold text-foreground-muted">
                    {finding.severity} · {finding.disposition}
                    {finding.file ? ` · ${finding.file}` : ""}
                  </Text>
                  <Text textRole="footnote" className="text-foreground">
                    {finding.message}
                  </Text>
                  {finding.resolutionNote ? (
                    <Text textRole="caption" className="text-foreground-secondary">
                      {finding.resolutionNote}
                    </Text>
                  ) : null}
                  {isOpen && props.canResolveFinding && card.status !== "completed" ? (
                    <View className="gap-2 pt-1">
                      <View className="flex-row flex-wrap gap-1.5">
                        {DISPOSITIONS.map((disp) => (
                          <Chip
                            key={disp}
                            size="small"
                            label={mobileFindingDispositionLabel(disp, messages)}
                            selected={draft.disposition === disp}
                            onPress={() =>
                              setFindingDrafts((current) => ({
                                ...current,
                                [finding.id]: { ...draft, disposition: disp },
                              }))
                            }
                          />
                        ))}
                      </View>
                      <AppTextInput
                        accessibilityLabel={gt("Finding resolution reason")}
                        placeholder={
                          draft.disposition === "dismiss" || draft.disposition === "accept_risk"
                            ? gt("Reason required for Dismiss or Accept risk")
                            : gt("Optional note")
                        }
                        value={draft.reason}
                        onChangeText={(reason) =>
                          setFindingDrafts((current) => ({
                            ...current,
                            [finding.id]: { ...draft, reason },
                          }))
                        }
                      />
                      <Button
                        label={gt("Resolve finding")}
                        size="compact"
                        tone="secondary"
                        disabled={props.busyAction !== null}
                        loading={props.busyAction === `finding:${finding.id}`}
                        onPress={() =>
                          props.onResolveFinding({
                            findingId: finding.id,
                            disposition: draft.disposition,
                            reason: draft.reason,
                          })
                        }
                      />
                    </View>
                  ) : null}
                </View>
              );
            })}
          </View>
        )}
      </SectionCard>

      <DiffSummaryCard
        cardDiff={props.cardDiff}
        loading={props.cardDiffLoading}
        error={props.cardDiffError}
        onRefresh={props.onRefreshDiff}
      />
    </View>
  );
}

const VERDICTS: readonly WorkflowBoardManualAcceptanceVerdict[] = [
  "pass",
  "fail",
  "not-applicable",
];

export function ShipInspectSection(props: {
  readonly card: WorkflowCardDetail;
  readonly cardDiff: WorkflowBoardGetCardDiffResult | null;
  readonly cardDiffLoading: boolean;
  readonly cardDiffError: string | null;
  readonly onRefreshDiff: () => void;
  readonly canRecordAcceptance: boolean;
  readonly canApproveDelivery: boolean;
  readonly busyAction: string | null;
  readonly onRecordAcceptance: (input: {
    readonly criterionId: string;
    readonly verdict: WorkflowBoardManualAcceptanceVerdict;
    readonly note: string;
    readonly evidenceRef: string;
  }) => void;
  readonly onApproveDelivery: (note: string) => void;
  readonly onNavigateToFix: (target: WorkflowBoardFixNavigation) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;
  const shipped = isWorkflowBoardCardShipped(card);
  const currentSnapshotDigest =
    card.shipReadiness?.productSnapshotDigest ?? props.cardDiff?.productSnapshotDigest ?? null;
  const blockers = card.shipReadiness?.blockers ?? [];
  const observationsByCriterion = new Map(
    (card.acObservations ?? []).map((obs) => [obs.criterionId, obs]),
  );
  const deliveryReceipt = card.deliveryReceipt ?? card.shipReadiness?.deliveryReceipt ?? null;
  const canApprove = canApproveWorkflowBoardDelivery({
    detail: card,
    canOperate: props.canApproveDelivery,
  });

  const [deliveryNote, setDeliveryNote] = useState("");
  const [acDrafts, setAcDrafts] = useState<
    Readonly<
      Record<
        string,
        {
          readonly verdict: WorkflowBoardManualAcceptanceVerdict;
          readonly note: string;
          readonly evidenceRef: string;
        }
      >
    >
  >({});

  const draftForCriterion = (criterionId: string) => {
    const existing = acDrafts[criterionId];
    if (existing !== undefined) return existing;
    const obs = observationsByCriterion.get(criterionId);
    return {
      verdict:
        obs?.status === "fail"
          ? ("fail" as const)
          : obs?.status === "not_applicable"
            ? ("not-applicable" as const)
            : ("pass" as const),
      note: obs?.observationNote ?? "",
      evidenceRef: obs?.evidenceRef ?? "",
    };
  };

  return (
    <View className="gap-4">
      <SectionCard>
        <View className="flex-row items-center justify-between gap-2">
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>Ship readiness</T>
          </Text>
          <Text textRole="caption" className="text-foreground-muted">
            {workflowBoardDeliveryStatusLabel(
              card.shipReadiness?.status ?? card.deliveryStatus,
              messages,
            )}
          </Text>
        </View>
        {blockers.length === 0 ? (
          <Text textRole="footnote" className="text-foreground-secondary">
            {shipped
              ? gt("All delivery checks and acceptance criteria are satisfied.")
              : gt("No Ship blockers remain.")}
          </Text>
        ) : (
          blockers.map((blocker) => (
            <View key={blocker.id} className="gap-2 rounded-xl bg-subtle p-3">
              <Text textRole="caption" className="font-legion-semibold text-danger-foreground">
                {blocker.kind}
              </Text>
              <Text textRole="footnote" className="text-foreground">
                {blocker.message}
              </Text>
              <Button
                label={gt("Go to fix")}
                size="compact"
                tone="secondary"
                onPress={() => props.onNavigateToFix(resolveFixPointerNavigation(blocker.pointer))}
              />
            </View>
          ))
        )}

        {deliveryReceipt !== null ? (
          <View className="gap-1 rounded-xl bg-subtle p-3">
            <Text textRole="caption" className="font-legion-semibold text-foreground">
              <T>Delivery receipt</T>
            </Text>
            <Text textRole="caption" className="text-foreground-secondary">
              <T>
                Approved rev <Var>{deliveryReceipt.cardRevision}</Var> by{" "}
                <Var>{deliveryReceipt.approvedBy}</Var> at <Var>{deliveryReceipt.approvedAt}</Var>
              </T>
            </Text>
            <Text textRole="caption" className="text-foreground-muted">
              {deliveryReceipt.acVerdictsSummary}
            </Text>
          </View>
        ) : null}

        {!shipped ? (
          <View className="gap-2.5 pt-1">
            <AppTextInput
              accessibilityLabel={gt("Delivery approval note")}
              placeholder={gt("Optional delivery note")}
              value={deliveryNote}
              editable={canApprove && props.busyAction === null}
              onChangeText={setDeliveryNote}
            />
            <Button
              label={
                props.busyAction === "approve-delivery"
                  ? gt("Approving delivery…")
                  : gt("Approve delivery")
              }
              tone="primary"
              icon="checkmark.circle"
              disabled={!canApprove || props.busyAction !== null}
              loading={props.busyAction === "approve-delivery"}
              onPress={() => props.onApproveDelivery(deliveryNote)}
            />
          </View>
        ) : null}
      </SectionCard>

      <SectionCard>
        <Text textRole="headline" className="font-legion-semibold text-foreground">
          {gt("Criterion observations ({count})", { count: card.spec.acceptanceCriteria.length })}
        </Text>
        {card.spec.acceptanceCriteria.map((criterion) => {
          const obs = observationsByCriterion.get(criterion.id);
          const stale = obs !== undefined && isAcObservationStale(obs, currentSnapshotDigest);
          const draft = draftForCriterion(criterion.id);
          return (
            <View key={criterion.id} className="gap-2 rounded-xl bg-subtle p-3">
              <View className="flex-row items-center justify-between gap-2">
                <Text textRole="caption" className="font-legion-semibold text-foreground">
                  {criterion.id}
                </Text>
                <Text
                  textRole="caption"
                  className={
                    stale ? "font-legion-medium text-warning-foreground" : "text-foreground-muted"
                  }
                >
                  {obs ? workflowBoardAcEvidenceStatusLabel(obs.status, messages) : gt("Pending")}
                  {stale && obs?.status !== "stale" ? ` (${gt("Stale")})` : ""}
                </Text>
              </View>
              <Text textRole="footnote" className="text-foreground">
                {criterion.text}
              </Text>
              {obs?.observationNote ? (
                <Text textRole="caption" className="text-foreground-secondary">
                  {obs.observationNote}
                </Text>
              ) : null}
              {!shipped && props.canRecordAcceptance && card.status !== "paused" ? (
                <View className="gap-2 pt-1">
                  <View className="flex-row flex-wrap gap-1.5">
                    {VERDICTS.map((verdict) => (
                      <Chip
                        key={verdict}
                        size="small"
                        label={mobileManualAcceptanceVerdictLabel(verdict, messages)}
                        selected={draft.verdict === verdict}
                        onPress={() =>
                          setAcDrafts((current) => ({
                            ...current,
                            [criterion.id]: { ...draft, verdict },
                          }))
                        }
                      />
                    ))}
                  </View>
                  <AppTextInput
                    accessibilityLabel={gt("Observation note for {criterionId}", {
                      criterionId: criterion.id,
                    })}
                    placeholder={
                      draft.verdict === "fail" || draft.verdict === "not-applicable"
                        ? gt("Note required for Fail or Not applicable")
                        : gt("Optional observation note")
                    }
                    value={draft.note}
                    onChangeText={(note) =>
                      setAcDrafts((current) => ({
                        ...current,
                        [criterion.id]: { ...draft, note },
                      }))
                    }
                  />
                  <AppTextInput
                    accessibilityLabel={gt("Evidence reference for {criterionId}", {
                      criterionId: criterion.id,
                    })}
                    placeholder={gt("Optional evidence reference")}
                    value={draft.evidenceRef}
                    onChangeText={(evidenceRef) =>
                      setAcDrafts((current) => ({
                        ...current,
                        [criterion.id]: { ...draft, evidenceRef },
                      }))
                    }
                  />
                  <Button
                    label={gt("Save observation")}
                    size="compact"
                    tone="secondary"
                    disabled={props.busyAction !== null}
                    loading={props.busyAction === `ac:${criterion.id}`}
                    onPress={() =>
                      props.onRecordAcceptance({
                        criterionId: criterion.id,
                        verdict: draft.verdict,
                        note: draft.note,
                        evidenceRef: draft.evidenceRef,
                      })
                    }
                  />
                </View>
              ) : null}
            </View>
          );
        })}
      </SectionCard>

      <DiffSummaryCard
        cardDiff={props.cardDiff}
        loading={props.cardDiffLoading}
        error={props.cardDiffError}
        onRefresh={props.onRefreshDiff}
      />
    </View>
  );
}

export function HistoryInspectSection(props: { readonly card: WorkflowCardDetail }) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;
  const revisions = card.history ?? card.revisions;
  const orderedRevisions = [...revisions].reverse();

  return (
    <View className="gap-4">
      {card.cliImport !== undefined ? (
        <SectionCard>
          <Text textRole="headline" className="font-legion-semibold text-foreground">
            <T>CLI import provenance</T>
          </Text>
          <Text textRole="caption" className="text-foreground-secondary">
            <T>
              Source spec: <Var>{card.cliImport.sourceSpecId}</Var> · Owner:{" "}
              <Var>{card.cliImport.owner}</Var>
            </T>
          </Text>
          <Text textRole="caption" className="text-foreground-muted" selectable>
            {card.cliImport.sourceCliDir} · {card.cliImport.importedAt}
          </Text>
          {card.cliImport.assurancePolicySummary ? (
            <Text textRole="footnote" className="text-foreground-secondary">
              {card.cliImport.assurancePolicySummary}
            </Text>
          ) : null}
          {card.cliImport.historicalReceipts.length > 0 ? (
            <View className="gap-1.5 pt-1">
              <Text textRole="footnote" className="font-legion-semibold text-foreground">
                <T>Historical CLI receipts</T>
              </Text>
              {card.cliImport.historicalReceipts.map((receipt) => (
                <View key={receipt.id} className="gap-0.5 rounded-xl bg-subtle p-3">
                  <Text textRole="caption" className="font-legion-medium text-foreground">
                    {workflowBoardCliImportHistoricalReceiptKindLabel(receipt.kind, messages)} ·{" "}
                    {gt("Historical")}
                  </Text>
                  <Text textRole="caption" className="text-foreground-secondary">
                    {receipt.summary}
                  </Text>
                  <Text textRole="caption" className="text-foreground-muted" selectable>
                    {receipt.sourcePath}
                  </Text>
                </View>
              ))}
            </View>
          ) : null}
          {card.cliImport.unsupportedPolicies.length > 0 ? (
            <View className="gap-1.5 pt-1">
              <Text textRole="footnote" className="font-legion-semibold text-foreground">
                <T>Adopted CLI policies</T>
              </Text>
              {card.cliImport.unsupportedPolicies.map((policy) => (
                <View key={policy.key} className="gap-0.5 rounded-xl bg-subtle p-3">
                  <Text textRole="caption" className="font-legion-medium text-foreground">
                    {policy.key} ·{" "}
                    {workflowBoardCliImportPolicyStatusLabel(policy.status, messages)}
                  </Text>
                  <Text textRole="caption" className="text-foreground-secondary">
                    {policy.summary}
                  </Text>
                </View>
              ))}
            </View>
          ) : null}
          {card.cliImport.missingLogNotes.length > 0 ? (
            <View className="gap-1.5 pt-1">
              <Text textRole="footnote" className="font-legion-semibold text-foreground">
                <T>Missing CLI check logs</T>
              </Text>
              {card.cliImport.missingLogNotes.map((note) => (
                <View
                  key={`${note.taskId ?? ""}:${note.relativePath}`}
                  className="gap-0.5 rounded-xl bg-subtle p-3"
                >
                  <Text
                    textRole="caption"
                    className="font-legion-medium text-foreground"
                    selectable
                  >
                    {note.relativePath}
                  </Text>
                  <Text textRole="caption" className="text-foreground-secondary">
                    {note.note}
                  </Text>
                </View>
              ))}
            </View>
          ) : null}
        </SectionCard>
      ) : null}

      <ListSection title={gt("Revision history ({count})", { count: orderedRevisions.length })}>
        {orderedRevisions.length === 0 ? (
          <View className="bg-grouped-card p-4">
            <Text textRole="footnote" className="text-foreground-muted">
              <T>No revisions recorded yet.</T>
            </Text>
          </View>
        ) : (
          orderedRevisions.map((entry, index) => (
            <View
              key={`${entry.revision}-${entry.createdAt}`}
              className={
                index > 0
                  ? "gap-1 border-t border-border-subtle bg-grouped-card p-4"
                  : "gap-1 bg-grouped-card p-4"
              }
            >
              <View className="flex-row items-center justify-between gap-2">
                <Text textRole="caption" className="font-legion-semibold text-foreground">
                  <T>
                    rev <Var>{entry.revision}</Var>
                  </T>{" "}
                  · {workflowBoardStageLabel(entry.stage, messages)} · {entry.kind}
                </Text>
                <Text textRole="caption" className="text-foreground-muted">
                  {entry.createdAt}
                </Text>
              </View>
              <Text textRole="footnote" className="text-foreground-secondary">
                {entry.summary}
              </Text>
            </View>
          ))
        )}
      </ListSection>
    </View>
  );
}

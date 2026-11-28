import { useAtomValue } from "@effect/atom-react";
import { StackActions, useNavigation, type StaticScreenProps } from "@react-navigation/native";
import { T, useGT, useMessages, Var } from "gt-react-native";
import { useState } from "react";
import { Pressable, ScrollView as HorizontalScrollView, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { EnvironmentId, ProjectId, WorkflowBoardCardSummary } from "@legioncode/contracts";

import { StatusPill, type StatusTone } from "../../components/StatusPill";
import { AppBar } from "../../components/ui/AppBar";
import { SymbolView } from "../../components/ui/AppSymbol";
import { AppText as Text } from "../../components/ui/AppText";
import { Button } from "../../components/ui/Button";
import { Chip } from "../../components/ui/Chip";
import { EmptyState } from "../../components/ui/EmptyState";
import { ListSection } from "../../components/ui/ListSection";
import { Notice } from "../../components/ui/Notice";
import { ScreenContent } from "../../components/ui/ScreenContent";
import { ScreenScrollView as ScrollView } from "../../components/ui/ScreenScrollView";
import { SegmentedControl } from "../../components/ui/SegmentedControl";
import { useProjects } from "../../state/entities";
import { useEnvironments } from "../../state/environments";
import { useEnvironmentQuery } from "../../state/query";
import { workflowBoard } from "../../state/workflowBoard";
import { openSettings } from "../settings/settings-navigation";
import {
  MOBILE_BOARD_STAGE_FILTERS,
  defaultMobileBoardStageFilter,
  groupWorkflowBoardCards,
  mobileBoardStageFilterLabel,
  resolveMobileWorkflowBoardTarget,
  selectMobileBoardStageCards,
  workflowBoardAvailabilityNotice,
  workflowBoardCardBlocker,
  workflowBoardCardNextAction,
  workflowBoardCardStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardNeedsRecovery,
  workflowBoardPlanApprovalLabel,
  workflowBoardPlanApprovalState,
  workflowBoardProjectNotFoundNotice,
  workflowBoardReadPermissionNotice,
  workflowBoardSpecApprovalLabel,
  workflowBoardSpecApprovalState,
  workflowBoardStageLabel,
  isWorkflowBoardCardShipped,
  type MobileBoardStageFilter,
  type MobileWorkflowBoardCandidate,
} from "./workflowBoard.logic";

export type WorkflowBoardRouteParams =
  | {
      readonly environmentId?: string | undefined;
      readonly projectId?: string | undefined;
      readonly stage?: string | undefined;
    }
  | undefined;

function cardStatusTone(status: WorkflowBoardCardSummary["status"], label: string): StatusTone {
  switch (status) {
    case "blocked":
      return {
        label,
        pillClassName: "bg-danger",
        textClassName: "text-danger-foreground",
      };
    case "paused":
      return {
        label,
        pillClassName: "bg-warning",
        textClassName: "text-warning-foreground",
      };
    case "completed":
      return {
        label,
        pillClassName: "bg-primary",
        textClassName: "text-primary-foreground",
      };
    case "active":
      return {
        label,
        pillClassName: "bg-subtle",
        textClassName: "text-foreground-secondary",
      };
  }
}

function approvalBadgeClassName(state: "approved" | "stale" | "unapproved"): {
  readonly pill: string;
  readonly text: string;
} {
  switch (state) {
    case "approved":
      return { pill: "bg-subtle-strong", text: "text-primary-text" };
    case "stale":
      return { pill: "bg-warning", text: "text-warning-foreground" };
    case "unapproved":
      return { pill: "bg-subtle", text: "text-foreground-muted" };
  }
}

function WorkflowBoardCardRow(props: {
  readonly card: WorkflowBoardCardSummary;
  readonly separated: boolean;
  readonly onPress: () => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const statusLabel = workflowBoardCardStatusLabel(props.card.status, messages);
  const blocker = workflowBoardCardBlocker(props.card, messages);
  const nextAction = workflowBoardCardNextAction(props.card, messages);
  const specState = workflowBoardSpecApprovalState(props.card);
  const specClasses = approvalBadgeClassName(specState);
  const planState = workflowBoardPlanApprovalState(props.card);
  const planClasses = approvalBadgeClassName(planState);
  const needsRecovery = workflowBoardNeedsRecovery(props.card);
  const shipped = isWorkflowBoardCardShipped(props.card);

  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={props.card.title}
      onPress={props.onPress}
      className={
        props.separated
          ? "gap-2.5 border-t border-border-subtle bg-grouped-card p-4 active:opacity-75"
          : "gap-2.5 bg-grouped-card p-4 active:opacity-75"
      }
    >
      <View className="flex-row items-start justify-between gap-3">
        <View className="min-w-0 flex-1 gap-1">
          <Text
            textRole="headline"
            className="font-legion-semibold text-foreground"
            numberOfLines={2}
          >
            {props.card.title}
          </Text>
          <Text textRole="caption" className="text-foreground-muted">
            {workflowBoardStageLabel(props.card.stage, messages)} ·{" "}
            <T>
              rev <Var>{props.card.revision}</Var>
            </T>
          </Text>
        </View>
        <View className="flex-row items-center gap-2">
          <StatusPill {...cardStatusTone(props.card.status, statusLabel)} size="compact" />
          <SymbolView
            name="chevron.right"
            size={14}
            tintColorClassName="accent-chevron"
            type="monochrome"
            weight="semibold"
          />
        </View>
      </View>

      {blocker !== null ? (
        <Text
          textRole="footnote"
          className="font-legion-medium text-danger-foreground"
          numberOfLines={2}
        >
          {blocker}
        </Text>
      ) : props.card.currentTask !== null ? (
        <Text textRole="footnote" className="text-foreground-secondary" numberOfLines={2}>
          {props.card.currentTask}
        </Text>
      ) : null}

      <Text textRole="footnote" className="text-foreground-muted" numberOfLines={1}>
        <T>
          Next: <Var>{nextAction}</Var>
        </T>
      </Text>

      <View className="flex-row flex-wrap items-center gap-1.5">
        <View className={`rounded-full px-2.5 py-0.5 ${specClasses.pill}`}>
          <Text className={`font-legion-medium text-xs ${specClasses.text}`}>
            {workflowBoardSpecApprovalLabel(props.card.specApprovalStatus, messages)}
          </Text>
        </View>
        {props.card.planApprovalStatus !== undefined && props.card.planApprovalStatus !== null ? (
          <View className={`rounded-full px-2.5 py-0.5 ${planClasses.pill}`}>
            <Text className={`font-legion-medium text-xs ${planClasses.text}`}>
              {workflowBoardPlanApprovalLabel(props.card.planApprovalStatus, messages)}
            </Text>
          </View>
        ) : null}
        {shipped ? (
          <View className="rounded-full bg-primary px-2.5 py-0.5">
            <Text className="font-legion-medium text-xs text-primary-foreground">
              {workflowBoardDeliveryStatusLabel("shipped", messages)}
            </Text>
          </View>
        ) : null}
        {needsRecovery ? (
          <View className="rounded-full bg-warning px-2.5 py-0.5">
            <Text className="font-legion-medium text-xs text-warning-foreground">
              {gt("Needs reconciliation")}
            </Text>
          </View>
        ) : null}
        {props.card.specApprovalStatus === "historical" ||
        props.card.planApprovalStatus === "historical" ? (
          <View className="rounded-full bg-subtle px-2.5 py-0.5">
            <Text className="font-legion-medium text-xs text-foreground-muted">
              {gt("CLI import")}
            </Text>
          </View>
        ) : null}
      </View>
    </Pressable>
  );
}

export function WorkflowBoardRouteScreen({ route }: StaticScreenProps<WorkflowBoardRouteParams>) {
  const gt = useGT();
  const messages = useMessages();
  const navigation = useNavigation();
  const insets = useSafeAreaInsets();
  const { isReady, environments } = useEnvironments();
  const projects = useProjects();

  const routeTargetKey = JSON.stringify([
    route?.params?.environmentId ?? "",
    route?.params?.projectId ?? "",
    route?.params?.stage ?? "",
  ]);
  const [selectedOverride, setSelectedOverride] = useState<{
    readonly routeKey: string;
    readonly environmentId: EnvironmentId;
    readonly projectId: ProjectId;
  } | null>(null);
  const [stageOverride, setStageOverride] = useState<{
    readonly scopeKey: string;
    readonly stage: MobileBoardStageFilter;
  } | null>(null);

  const activeOverride = selectedOverride?.routeKey === routeTargetKey ? selectedOverride : null;
  const requestedEnvironmentId = activeOverride
    ? activeOverride.environmentId
    : route?.params?.environmentId;
  const requestedProjectId = activeOverride ? activeOverride.projectId : route?.params?.projectId;

  const target = resolveMobileWorkflowBoardTarget({
    isReady,
    environments,
    projects,
    requestedEnvironmentId,
    requestedProjectId,
  });

  const activeCandidate = target.selected;
  const activeEnvironmentId = activeCandidate?.environmentId ?? null;
  const activeProjectId = activeCandidate?.projectId ?? null;
  const activeScopeKey = activeCandidate
    ? JSON.stringify([activeCandidate.environmentId, activeCandidate.projectId])
    : routeTargetKey;

  const canReadBoard = useAtomValue(workflowBoard.readPermissionAtom(activeEnvironmentId));
  const boardEnabled =
    !target.loading &&
    !target.missingRequestedProject &&
    target.availability === "enabled" &&
    activeEnvironmentId !== null &&
    activeProjectId !== null;

  const snapshotQuery = useEnvironmentQuery(
    boardEnabled && canReadBoard
      ? workflowBoard.boardSnapshot({
          environmentId: activeEnvironmentId,
          input: { projectId: activeProjectId },
        })
      : null,
  );

  const selectCandidate = (candidate: MobileWorkflowBoardCandidate) => {
    setSelectedOverride({
      routeKey: routeTargetKey,
      environmentId: candidate.environmentId,
      projectId: candidate.projectId,
    });
    setStageOverride(null);
  };

  const columns = groupWorkflowBoardCards(snapshotQuery.data?.cards ?? []);
  const selectedStage: MobileBoardStageFilter =
    stageOverride !== null && stageOverride.scopeKey === activeScopeKey
      ? stageOverride.stage
      : defaultMobileBoardStageFilter(columns, route?.params?.stage);
  const visibleCards = selectMobileBoardStageCards(columns, selectedStage);

  const loadingNotice = workflowBoardAvailabilityNotice("loading", messages);
  const availabilityNotice = workflowBoardAvailabilityNotice(target.availability, messages);
  const readPermissionNotice = workflowBoardReadPermissionNotice(messages);
  const projectNotFoundNotice = workflowBoardProjectNotFoundNotice(messages);
  const snapshotErrorText =
    snapshotQuery.failure !== null
      ? (workflowBoardErrorMessage(snapshotQuery.failure, messages) ??
        snapshotQuery.error ??
        gt("Could not load workflow board."))
      : null;

  return (
    <View className="flex-1 bg-screen">
      <AppBar
        title={gt("Workflow board")}
        subtitle={
          activeCandidate
            ? `${activeCandidate.projectTitle} · ${activeCandidate.environmentLabel}`
            : null
        }
        onBack={navigation.canGoBack() ? () => navigation.goBack() : undefined}
        refresh={
          boardEnabled && canReadBoard
            ? {
                accessibilityLabel: gt("Refresh workflow board"),
                disabled: snapshotQuery.isPending,
                onPress: () => snapshotQuery.refresh(),
              }
            : undefined
        }
      />
      <ScreenContent surface="screen">
        <ScrollView
          className="flex-1"
          contentInsetAdjustmentBehavior="automatic"
          showsVerticalScrollIndicator={false}
          contentContainerClassName="gap-5 px-5 pt-4"
          contentContainerStyle={{ paddingBottom: Math.max(insets.bottom, 20) + 16 }}
        >
          {target.candidates.length > 1 ? (
            <View className="gap-2">
              <Text textRole="footnote" className="px-1 font-legion-medium text-foreground-muted">
                <T>Project checkout</T>
              </Text>
              <HorizontalScrollView
                horizontal
                showsHorizontalScrollIndicator={false}
                contentContainerStyle={{ gap: 8 }}
              >
                {target.candidates.map((candidate) => (
                  <Chip
                    key={JSON.stringify([candidate.environmentId, candidate.projectId])}
                    label={`${candidate.projectTitle} (${candidate.environmentLabel})`}
                    selected={
                      activeCandidate?.environmentId === candidate.environmentId &&
                      activeCandidate?.projectId === candidate.projectId
                    }
                    onPress={() => selectCandidate(candidate)}
                  />
                ))}
              </HorizontalScrollView>
            </View>
          ) : null}

          {target.loading || target.availability === "loading" ? (
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
          ) : snapshotErrorText !== null && snapshotQuery.data === null ? (
            <View className="gap-3">
              <Notice tone="danger" message={snapshotErrorText} />
              <Button
                label={gt("Retry")}
                tone="secondary"
                onPress={() => snapshotQuery.refresh()}
              />
            </View>
          ) : snapshotQuery.isPending && snapshotQuery.data === null ? (
            <EmptyState
              title={gt("Loading workflow board…")}
              detail={gt("Fetching workflow cards for this project.")}
            />
          ) : (
            <>
              {snapshotErrorText !== null ? (
                <Notice tone="danger" message={snapshotErrorText} />
              ) : null}
              <SegmentedControl
                options={MOBILE_BOARD_STAGE_FILTERS.map((stage) => ({
                  value: stage,
                  label: mobileBoardStageFilterLabel(stage, messages),
                }))}
                selected={selectedStage}
                onSelect={(stage) => setStageOverride({ scopeKey: activeScopeKey, stage })}
              />

              {visibleCards.length === 0 ? (
                <EmptyState
                  title={gt("No cards in {stage}", {
                    stage: mobileBoardStageFilterLabel(selectedStage, messages),
                  })}
                  detail={
                    selectedStage === "completed"
                      ? gt("Shipped workflow cards appear here once delivery is approved.")
                      : gt("Cards in this stage will appear here as work progresses.")
                  }
                />
              ) : (
                <ListSection
                  title={gt("{stage} ({count})", {
                    stage: mobileBoardStageFilterLabel(selectedStage, messages),
                    count: visibleCards.length,
                  })}
                >
                  {visibleCards.map((card, index) => (
                    <WorkflowBoardCardRow
                      key={card.cardId}
                      card={card}
                      separated={index > 0}
                      onPress={() =>
                        navigation.dispatch(
                          StackActions.push("WorkflowCard", {
                            environmentId: String(activeEnvironmentId),
                            projectId: String(activeProjectId),
                            cardId: String(card.cardId),
                            stage: card.stage,
                          }),
                        )
                      }
                    />
                  ))}
                </ListSection>
              )}
            </>
          )}
        </ScrollView>
      </ScreenContent>
    </View>
  );
}

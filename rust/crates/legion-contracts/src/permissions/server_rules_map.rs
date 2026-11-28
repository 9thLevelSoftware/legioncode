//! Exhaustive static server-side scope mapping for all 257 `WsMethod` variants
//! and `Custom(...)` Wave D/H + `workflowBoard.*` RPCs (`RpcAuthorization.ts`).

use super::super::{EnvironmentScope, WsMethod};

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive match over all 257 WsMethod variants"
)]
pub(super) fn static_server_scope(method: &WsMethod) -> Option<EnvironmentScope> {
    match method {
        WsMethod::ProjectsListEntries
        | WsMethod::ProjectsListIgnoredFiles
        | WsMethod::ProjectsReadFile
        | WsMethod::ProjectsSearchContents
        | WsMethod::ProjectsSearchEntries
        | WsMethod::FilesystemBrowse
        | WsMethod::FilesystemGetMetadata
        | WsMethod::WorkspaceListFolders
        | WsMethod::WorkspaceScanFolders
        | WsMethod::WorkspaceTaskFolder
        | WsMethod::ReviewGetBranchCommits
        | WsMethod::ReviewGetDiffPreview
        | WsMethod::ReviewGetDiffFileContents => Some(EnvironmentScope::FilesystemRead),

        WsMethod::ProjectsWriteFile => Some(EnvironmentScope::FilesystemWrite),

        WsMethod::TerminalObserve | WsMethod::SubscribeTerminalMetadata => {
            Some(EnvironmentScope::TerminalRead)
        }
        WsMethod::TerminalOpen
        | WsMethod::TerminalAttach
        | WsMethod::TerminalWrite
        | WsMethod::TerminalResize
        | WsMethod::TerminalClear
        | WsMethod::TerminalClose
        | WsMethod::PreviewStopLocalServer
        | WsMethod::WorkflowsSaveDefinition
        | WsMethod::WorkflowsDeleteDefinition => Some(EnvironmentScope::TerminalOperate),

        WsMethod::PreviewOpen
        | WsMethod::PreviewResize
        | WsMethod::PreviewAdjust
        | WsMethod::PreviewClose
        | WsMethod::PreviewClearProfile
        | WsMethod::PreviewReportProfiles
        | WsMethod::PreviewReportStatus => Some(EnvironmentScope::PreviewOperate),

        WsMethod::ServerGetTraceDiagnostics
        | WsMethod::ServerGetProcessDiagnostics
        | WsMethod::ServerGetProcessResourceHistory
        | WsMethod::ServerGetResourceTelemetryHistory
        | WsMethod::ServerRetryResourceTelemetry
        | WsMethod::ServerGetUsageSummary
        | WsMethod::ServerRefreshUsageRates
        | WsMethod::SubscribeResourceTelemetry => Some(EnvironmentScope::DiagnosticsRead),

        WsMethod::ServerUpdateServer
        | WsMethod::ServerUpdateServerWithProgress
        | WsMethod::ServerCommitDesktopUpdate
        | WsMethod::ServerReclaimDatabaseSpace
        | WsMethod::ServerSignalProcess => Some(EnvironmentScope::EnvironmentMaintain),

        WsMethod::ProviderMcpSignInStart
        | WsMethod::ProviderMcpSignInComplete
        | WsMethod::ProviderAuthStart
        | WsMethod::ProviderConsumeResetCredit
        | WsMethod::ProviderAuthComplete
        | WsMethod::ChatGptReconnectProfile
        | WsMethod::ChatGptImportProfile
        | WsMethod::ChatGptHandoffSubscribe
        | WsMethod::ProviderAuthRespond
        | WsMethod::ProviderAuthCancel
        | WsMethod::ProviderAuthRemoveMethod
        | WsMethod::ProviderAuthLogout
        | WsMethod::ProviderAuthSubscribe
        | WsMethod::ProviderInstallStart
        | WsMethod::ProviderInstallCancel
        | WsMethod::ProviderInstallRemove
        | WsMethod::ServerUpdateProvider
        | WsMethod::ServerPrepareAcpRegistryAgent
        | WsMethod::ServerUninstallAcpRegistryManagedBinary
        | WsMethod::ServerAcceptAcpRegistryUrlAuth
        | WsMethod::ServerSetAcpRegistryProvider
        | WsMethod::ServerDisableAcpRegistryProvider
        | WsMethod::ServerLogoutAcpRegistry => Some(EnvironmentScope::ProvidersManage),

        WsMethod::DeviceConfigure
        | WsMethod::DeviceTestHost
        | WsMethod::DeviceEasAccount
        | WsMethod::DeviceSimlockIdentity
        | WsMethod::ServerUpsertKeybinding
        | WsMethod::ServerRemoveKeybinding
        | WsMethod::ServerRunStorageCleanup
        | WsMethod::ServerUpdateSettings
        | WsMethod::ServerCheckSecretSource
        | WsMethod::ServerCheckOtlpEndpoint
        | WsMethod::ServerListTextGeneratorModels
        | WsMethod::IssueTrackersConnect
        | WsMethod::IssueTrackersBindProject
        | WsMethod::SourceControlSignInStart
        | WsMethod::SourceControlSignInWait
        | WsMethod::SourceControlSignInCancel => Some(EnvironmentScope::SettingsWrite),

        WsMethod::WorkspaceAddTaskFolderRepository
        | WsMethod::VcsPull
        | WsMethod::VcsCreateWorktree
        | WsMethod::VcsRemoveWorktree
        | WsMethod::VcsCreateRef
        | WsMethod::VcsSwitchRef
        | WsMethod::VcsInit
        | WsMethod::GitRunStackedAction
        | WsMethod::GitPreparePullRequestThread
        | WsMethod::PullRequestsRunAction
        | WsMethod::PullRequestsUpdate
        | WsMethod::PullRequestsComment
        | WsMethod::PullRequestsUpdateComment
        | WsMethod::PullRequestsSubmitReview
        | WsMethod::PullRequestsReplyToThread
        | WsMethod::PullRequestsSetThreadResolution
        | WsMethod::PullRequestsSetReaction
        | WsMethod::PullRequestsSetFilesViewed
        | WsMethod::PullRequestsRequestReviewers
        | WsMethod::PullRequestsSetLabels
        | WsMethod::IssuesCreate
        | WsMethod::IssuesComment
        | WsMethod::IssuesUpdate
        | WsMethod::IssuesSetState
        | WsMethod::IssuesReact
        | WsMethod::SourceControlCloneRepository
        | WsMethod::SourceControlPublishRepository
        | WsMethod::SourceControlStar
        | WsMethod::ProjectCloneStart
        | WsMethod::ProjectCloneCancel
        | WsMethod::ProjectCloneRetry => Some(EnvironmentScope::SourceControlWrite),

        WsMethod::PeerLinksList | WsMethod::SubscribeAuthAccess => {
            Some(EnvironmentScope::AccessRead)
        }
        WsMethod::GuestShareCreate
        | WsMethod::PeerLinksLink
        | WsMethod::PeerLinksUnlink
        | WsMethod::PeerLinksAnswerRequest
        | WsMethod::PeerLinksPairClient
        | WsMethod::PeerLinksPairLink => Some(EnvironmentScope::AccessWrite),

        WsMethod::CloudGetRelayClientStatus => Some(EnvironmentScope::RelayRead),
        WsMethod::CloudInstallRelayClient => Some(EnvironmentScope::RelayWrite),

        WsMethod::AgentSessionsScan
        | WsMethod::AssetsCreateUrl
        | WsMethod::McpAppsToolInfo
        | WsMethod::McpAppsReadResource
        | WsMethod::ProviderInstallSubscribe
        | WsMethod::VcsRefreshStatus
        | WsMethod::VcsListRefs
        | WsMethod::GitResolvePullRequest
        | WsMethod::PreviewList
        | WsMethod::DeviceList
        | WsMethod::DeviceDetail
        | WsMethod::ServerProbe
        | WsMethod::ServerGetConfig
        | WsMethod::ServerGetNpmRelease
        | WsMethod::ServerRefreshProviders
        | WsMethod::ServerGetStorageCleanupReport
        | WsMethod::ServerGetDatabaseStorage
        | WsMethod::ServerGetSettings
        | WsMethod::ServerDiscoverSourceControl
        | WsMethod::ServerSearchAcpRegistry
        | WsMethod::ServerListAcpRegistrySessions
        | WsMethod::ServerListAcpRegistryProviders
        | WsMethod::ServerGetHostResources
        | WsMethod::ServerReportClientActivity
        | WsMethod::PeerLinksNames
        | WsMethod::PeerLinksSubscribeThread
        | WsMethod::PeerLinksThreadHistoryPage
        | WsMethod::PeerLinksThreadItem
        | WsMethod::ScheduledTasksSubscribe
        | WsMethod::PullRequestsList
        | WsMethod::PullRequestsListStats
        | WsMethod::PullRequestsFilterOptions
        | WsMethod::PullRequestsSummary
        | WsMethod::PullRequestsRouting
        | WsMethod::PullRequestsRoutingIdentity
        | WsMethod::PullRequestsStack
        | WsMethod::PullRequestsLinkedThreads
        | WsMethod::PullRequestsDetail
        | WsMethod::PullRequestsPreview
        | WsMethod::PullRequestsChecks
        | WsMethod::PullRequestsActivity
        | WsMethod::PullRequestsReviewGuide
        | WsMethod::PullRequestsThreadComments
        | WsMethod::PullRequestsDiffFileContents
        | WsMethod::PullRequestsFilesViewed
        | WsMethod::PullRequestsChangeBreakdown
        | WsMethod::PullRequestsInvalidate
        | WsMethod::PullRequestsReportState
        | WsMethod::PullRequestsSubscribeRefreshes
        | WsMethod::PullRequestsSubscribeLinkedThreadRefreshes
        | WsMethod::PullRequestsReviewerCandidates
        | WsMethod::PullRequestsLabelCandidates
        | WsMethod::IssuesList
        | WsMethod::IssuesDetail
        | WsMethod::IssuesComments
        | WsMethod::IssuesTemplates
        | WsMethod::IssuesCandidates
        | WsMethod::IssuesLinkedThreads
        | WsMethod::IssueTrackersList
        | WsMethod::SourceControlLookupRepository
        | WsMethod::SourceControlAppAccess
        | WsMethod::SourceControlStarred
        | WsMethod::SubscribeProjectClones
        | WsMethod::SubscribeVcsStatus
        | WsMethod::SubscribeVcsBranch
        | WsMethod::VcsRefreshBranch
        | WsMethod::SubscribeWorktreeSetup
        | WsMethod::SubscribePreviewEvents
        | WsMethod::SubscribeDiscoveredLocalServers
        | WsMethod::SubscribeDeviceState
        | WsMethod::SubscribeServerConfig
        | WsMethod::SubscribeServerLifecycle
        | WsMethod::OrchestrationGetTurnDiff
        | WsMethod::OrchestrationGetFullThreadDiff
        | WsMethod::OrchestrationSearchThreads
        | WsMethod::OrchestrationSearchThread
        | WsMethod::OrchestrationSearchThreadStream
        | WsMethod::OrchestrationGetArchivedShellSnapshot
        | WsMethod::OrchestrationGetThreadFamily
        | WsMethod::OrchestrationGetThreadTitles
        | WsMethod::OrchestrationGetThreadLinkMetadata
        | WsMethod::OrchestrationGetWorktreeThreadIds
        | WsMethod::OrchestrationGetOlderSettledThreads
        | WsMethod::OrchestrationGetThreadProjection
        | WsMethod::OrchestrationGetWorkflowScript
        | WsMethod::OrchestrationGetNativeChildWork
        | WsMethod::OrchestrationGetTurnItem
        | WsMethod::OrchestrationSubscribeShell
        | WsMethod::OrchestrationSubscribeThread
        | WsMethod::OrchestrationSubscribeCommandOutput
        | WsMethod::OrchestrationSubscribePromptSuggestion
        | WsMethod::WorkflowsSubscribe
        | WsMethod::WorkflowsCounts
        | WsMethod::WorkflowsGet
        | WsMethod::WorkflowsGetDefinition
        | WsMethod::WorkflowBoardSubscribe
        | WsMethod::WorkflowBoardGetCard
        | WsMethod::WorkflowBoardGetArtifact
        | WsMethod::WorkflowBoardGetCardDiff => Some(EnvironmentScope::OrchestrationRead),

        WsMethod::ProjectsMutate
        | WsMethod::ProjectsEnsureScratch
        | WsMethod::ProjectsCreateNew
        | WsMethod::ProjectsCreateFromThread
        | WsMethod::ShellOpenInEditor
        | WsMethod::AgentSessionsImport
        | WsMethod::AssetsPersistChatAttachments
        | WsMethod::AttachmentsCreateUploadUrl
        | WsMethod::AttachmentsDelete
        | WsMethod::ProviderUploadFeedback
        | WsMethod::McpAppsCallTool
        | WsMethod::McpAppsUpdateModelContext
        | WsMethod::DeviceOpen
        | WsMethod::DeviceClose
        | WsMethod::DeviceShutdown
        | WsMethod::DeviceAction
        | WsMethod::ServerImportAcpRegistrySession
        | WsMethod::ServerDeleteAcpRegistrySession
        | WsMethod::ServerPrepareTriage
        | WsMethod::ScheduledTasksUpsert
        | WsMethod::ScheduledTasksSetEnabled
        | WsMethod::ScheduledTasksDelete
        | WsMethod::ScheduledTasksRunNow
        | WsMethod::ScheduledTasksRotateWebhookToken
        | WsMethod::SecretsAnswerRequest
        | WsMethod::ScheduledTasksListWebhookDeliveries
        | WsMethod::ScheduledTasksGetWebhookDelivery
        | WsMethod::PullRequestsGenerateReviewGuide
        | WsMethod::WorktreeSetupCancel
        | WsMethod::OrchestrationDispatchCommand
        | WsMethod::OrchestrationStopWorkflow
        | WsMethod::OrchestrationForceStopThread
        | WsMethod::OrchestrationLaunchThread
        | WsMethod::WorkflowsAnswer
        | WsMethod::WorkflowsResume
        | WsMethod::WorkflowsCancel
        | WsMethod::WorkflowBoardCreateCard
        | WsMethod::WorkflowBoardUpdateSpec
        | WsMethod::WorkflowBoardApproveSpec
        | WsMethod::WorkflowBoardStartStage
        | WsMethod::WorkflowBoardExploreStep
        | WsMethod::WorkflowBoardChallengeStep
        | WsMethod::WorkflowBoardUpdatePlan
        | WsMethod::WorkflowBoardApprovePlan
        | WsMethod::WorkflowBoardReturnForRevision
        | WsMethod::WorkflowBoardPauseCard
        | WsMethod::WorkflowBoardResumeCard
        | WsMethod::WorkflowBoardRetryStep
        | WsMethod::WorkflowBoardProposeAmendment
        | WsMethod::WorkflowBoardResolveFinding
        | WsMethod::WorkflowBoardRecordManualAcceptance
        | WsMethod::WorkflowBoardApproveDelivery
        | WsMethod::WorkflowBoardPreviewCliImport
        | WsMethod::WorkflowBoardApplyCliImport
        | WsMethod::AgentFeedbackAnswerRequest => Some(EnvironmentScope::OrchestrationOperate),

        WsMethod::Custom(raw) => static_custom_scope(raw),
    }
}

fn static_custom_scope(raw: &str) -> Option<EnvironmentScope> {
    match raw {
        "orchestration.getShellSnapshot"
        | "providers.listDrivers"
        | "checkpoints.getDiff"
        | "themes.read"
        | "surfaces.decorateDiff"
        | "surfaces.mobileChrome"
        | "workflowBoard.listCards" => Some(EnvironmentScope::OrchestrationRead),

        "diagnostics.traceSummary" | "resources.sample" => Some(EnvironmentScope::DiagnosticsRead),
        "terminal.snapshot" => Some(EnvironmentScope::TerminalRead),

        "workflowBoard.startPlan" | "workflowBoard.ship" => {
            Some(EnvironmentScope::OrchestrationOperate)
        }

        _ => None,
    }
}

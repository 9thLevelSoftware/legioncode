//! WebSocket RPC method identifiers covering all 266 known methods of `WsRpcGroup` plus unknown custom tags.

use thiserror::Error;

/// Error returned when parsing an empty WebSocket RPC method name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("WebSocket RPC method name cannot be empty")]
pub struct EmptyWsMethodError;

macro_rules! define_ws_methods {
    (
        g1: [ $(($v1:ident, $w1:literal)),* $(,)? ],
        g2: [ $(($v2:ident, $w2:literal)),* $(,)? ],
        g3: [ $(($v3:ident, $w3:literal)),* $(,)? ],
        g4: [ $(($v4:ident, $w4:literal)),* $(,)? ] $(,)?
    ) => {
        /// Typed WebSocket RPC method tag covering `WsRpcGroup` (`rpc.ts` & `orchestrationV2.ts`).
        ///
        /// Unrecognized non-empty method tags decode into [`WsMethod::Custom`] so forward-compatible
        /// callers never fail on newer server methods.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum WsMethod {
            $(
                #[doc = concat!("WebSocket RPC method `", $w1, "`.")]
                $v1,
            )*
            $(
                #[doc = concat!("WebSocket RPC method `", $w2, "`.")]
                $v2,
            )*
            $(
                #[doc = concat!("WebSocket RPC method `", $w3, "`.")]
                $v3,
            )*
            $(
                #[doc = concat!("WebSocket RPC method `", $w4, "`.")]
                $v4,
            )*
            /// Any unrecognized non-empty WebSocket RPC method tag.
            Custom(String),
        }

        impl WsMethod {
            fn as_str_g1(&self) -> Option<&'static str> {
                match self { $(Self::$v1 => Some($w1),)* _ => None }
            }
            fn as_str_g2(&self) -> Option<&'static str> {
                match self { $(Self::$v2 => Some($w2),)* _ => None }
            }
            fn as_str_g3(&self) -> Option<&'static str> {
                match self { $(Self::$v3 => Some($w3),)* _ => None }
            }
            fn as_str_g4(&self) -> Option<&'static str> {
                match self { $(Self::$v4 => Some($w4),)* _ => None }
            }

            fn from_known_g1(wire: &str) -> Option<Self> {
                match wire { $($w1 => Some(Self::$v1),)* _ => None }
            }
            fn from_known_g2(wire: &str) -> Option<Self> {
                match wire { $($w2 => Some(Self::$v2),)* _ => None }
            }
            fn from_known_g3(wire: &str) -> Option<Self> {
                match wire { $($w3 => Some(Self::$v3),)* _ => None }
            }
            fn from_known_g4(wire: &str) -> Option<Self> {
                match wire { $($w4 => Some(Self::$v4),)* _ => None }
            }
        }
    };
}

impl WsMethod {
    /// Parses a non-empty RPC method tag into a known [`WsMethod`] variant or [`WsMethod::Custom`].
    pub fn parse(raw: &str) -> Result<Self, EmptyWsMethodError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(EmptyWsMethodError);
        }
        Ok(Self::from_known_g1(trimmed)
            .or_else(|| Self::from_known_g2(trimmed))
            .or_else(|| Self::from_known_g3(trimmed))
            .or_else(|| Self::from_known_g4(trimmed))
            .unwrap_or_else(|| Self::Custom(trimmed.to_owned())))
    }

    /// Returns the canonical wire method string.
    pub fn as_str(&self) -> &str {
        if let Self::Custom(other) = self {
            return other;
        }
        self.as_str_g1()
            .or_else(|| self.as_str_g2())
            .or_else(|| self.as_str_g3())
            .or_else(|| self.as_str_g4())
            .unwrap_or("")
    }
}

define_ws_methods!(
    g1: [
        (ProjectsListEntries, "projects.listEntries"),
        (ProjectsListIgnoredFiles, "projects.listIgnoredFiles"),
        (ProjectsReadFile, "projects.readFile"),
        (ProjectsSearchContents, "projects.searchContents"),
        (ProjectsSearchEntries, "projects.searchEntries"),
        (ProjectsWriteFile, "projects.writeFile"),
        (ProjectsMutate, "projects.mutate"),
        (ProjectsEnsureScratch, "projects.ensureScratch"),
        (ProjectsCreateNew, "projects.createNew"),
        (ProjectsCreateFromThread, "projects.createFromThread"),
        (ShellOpenInEditor, "shell.openInEditor"),
        (FilesystemBrowse, "filesystem.browse"),
        (FilesystemGetMetadata, "filesystem.getMetadata"),
        (WorkspaceListFolders, "workspace.listFolders"),
        (WorkspaceScanFolders, "workspace.scanFolders"),
        (WorkspaceTaskFolder, "workspace.taskFolder"),
        (WorkspaceAddTaskFolderRepository, "workspace.addTaskFolderRepository"),
        (AgentSessionsScan, "agentSessions.scan"),
        (AgentSessionsImport, "agentSessions.import"),
        (AssetsCreateUrl, "assets.createUrl"),
        (AssetsPersistChatAttachments, "assets.persistChatAttachments"),
        (AttachmentsCreateUploadUrl, "attachments.createUploadUrl"),
        (AttachmentsDelete, "attachments.delete"),
        (McpAppsCallTool, "mcpApps.callTool"),
        (McpAppsToolInfo, "mcpApps.toolInfo"),
        (McpAppsReadResource, "mcpApps.readResource"),
        (McpAppsUpdateModelContext, "mcpApps.updateModelContext"),
        (ProviderUploadFeedback, "provider.uploadFeedback"),
        (ProviderMcpSignInStart, "provider.mcpSignIn.start"),
        (ProviderMcpSignInComplete, "provider.mcpSignIn.complete"),
        (ProviderAuthStart, "provider.auth.start"),
        (ProviderConsumeResetCredit, "provider.consumeResetCredit"),
        (ProviderAuthComplete, "provider.auth.complete"),
        (ChatGptReconnectProfile, "provider.chatgpt.reconnect-profile"),
        (ChatGptImportProfile, "provider.chatgpt.import-profile"),
        (ChatGptHandoffSubscribe, "provider.chatgpt.handoff.subscribe"),
        (ProviderAuthRespond, "provider.auth.respond"),
        (ProviderAuthCancel, "provider.auth.cancel"),
        (ProviderAuthRemoveMethod, "provider.auth.removeMethod"),
        (ProviderAuthLogout, "provider.auth.logout"),
        (ProviderAuthSubscribe, "provider.auth.subscribe"),
        (ProviderInstallStart, "provider.install.start"),
        (ProviderInstallCancel, "provider.install.cancel"),
        (ProviderInstallSubscribe, "provider.install.subscribe"),
        (ProviderInstallRemove, "provider.install.remove"),
        (VcsPull, "vcs.pull"),
        (VcsRefreshStatus, "vcs.refreshStatus"),
        (VcsListRefs, "vcs.listRefs"),
        (VcsCreateWorktree, "vcs.createWorktree"),
        (VcsRemoveWorktree, "vcs.removeWorktree"),
        (VcsCreateRef, "vcs.createRef"),
        (VcsSwitchRef, "vcs.switchRef"),
        (VcsInit, "vcs.init"),
        (GitRunStackedAction, "git.runStackedAction"),
        (GitResolvePullRequest, "git.resolvePullRequest"),
        (GitPreparePullRequestThread, "git.preparePullRequestThread"),
        (ReviewGetBranchCommits, "review.getBranchCommits"),
        (ReviewGetDiffPreview, "review.getDiffPreview"),
        (ReviewGetDiffFileContents, "review.getDiffFileContents"),
        (TerminalOpen, "terminal.open"),
        (TerminalAttach, "terminal.attach"),
        (TerminalObserve, "terminal.observe"),
        (TerminalWrite, "terminal.write"),
        (TerminalResize, "terminal.resize"),
        (TerminalClear, "terminal.clear"),
        (TerminalClose, "terminal.close"),
    ],
    g2: [
        (PreviewOpen, "preview.open"),
        (PreviewResize, "preview.resize"),
        (PreviewAdjust, "preview.adjust"),
        (PreviewClose, "preview.close"),
        (PreviewList, "preview.list"),
        (PreviewClearProfile, "preview.clearProfile"),
        (PreviewReportProfiles, "preview.reportProfiles"),
        (PreviewReportStatus, "preview.reportStatus"),
        (PreviewStopLocalServer, "preview.stopLocalServer"),
        (DeviceConfigure, "device.configure"),
        (DeviceList, "device.list"),
        (DeviceTestHost, "device.testHost"),
        (DeviceEasAccount, "device.easAccount"),
        (DeviceSimlockIdentity, "device.simlockIdentity"),
        (DeviceOpen, "device.open"),
        (DeviceClose, "device.close"),
        (DeviceShutdown, "device.shutdown"),
        (DeviceDetail, "device.detail"),
        (DeviceAction, "device.action"),
        (ServerProbe, "server.probe"),
        (ServerGetConfig, "server.getConfig"),
        (ServerGetNpmRelease, "server.getNpmRelease"),
        (ServerRefreshProviders, "server.refreshProviders"),
        (ServerUpdateProvider, "server.updateProvider"),
        (ServerUpdateServer, "server.updateServer"),
        (ServerUpdateServerWithProgress, "server.updateServerWithProgress"),
        (ServerCommitDesktopUpdate, "server.commitDesktopUpdate"),
        (ServerUpsertKeybinding, "server.upsertKeybinding"),
        (ServerRemoveKeybinding, "server.removeKeybinding"),
        (ServerRunStorageCleanup, "server.runStorageCleanup"),
        (ServerGetStorageCleanupReport, "server.getStorageCleanupReport"),
        (ServerGetDatabaseStorage, "server.getDatabaseStorage"),
        (ServerReclaimDatabaseSpace, "server.reclaimDatabaseSpace"),
        (PeerLinksList, "peerLinks.list"),
        (PeerLinksNames, "peerLinks.names"),
        (PeerLinksLink, "peerLinks.link"),
        (PeerLinksUnlink, "peerLinks.unlink"),
        (PeerLinksAnswerRequest, "peerLinks.answerRequest"),
        (PeerLinksSubscribeThread, "peerLinks.subscribeThread"),
        (PeerLinksThreadHistoryPage, "peerLinks.threadHistoryPage"),
        (PeerLinksThreadItem, "peerLinks.threadItem"),
        (PeerLinksPairClient, "peerLinks.pairClient"),
        (PeerLinksPairLink, "peerLinks.pairLink"),
        (ServerGetSettings, "server.getSettings"),
        (ServerUpdateSettings, "server.updateSettings"),
        (ServerDiscoverSourceControl, "server.discoverSourceControl"),
        (ServerCheckSecretSource, "server.checkSecretSource"),
        (ServerSearchAcpRegistry, "server.searchAcpRegistry"),
        (ServerPrepareAcpRegistryAgent, "server.prepareAcpRegistryAgent"),
        (ServerUninstallAcpRegistryManagedBinary, "server.uninstallAcpRegistryManagedBinary"),
        (ServerAcceptAcpRegistryUrlAuth, "server.acceptAcpRegistryUrlAuth"),
        (ServerListAcpRegistrySessions, "server.listAcpRegistrySessions"),
        (ServerImportAcpRegistrySession, "server.importAcpRegistrySession"),
        (ServerDeleteAcpRegistrySession, "server.deleteAcpRegistrySession"),
        (ServerListAcpRegistryProviders, "server.listAcpRegistryProviders"),
        (ServerSetAcpRegistryProvider, "server.setAcpRegistryProvider"),
        (ServerDisableAcpRegistryProvider, "server.disableAcpRegistryProvider"),
        (ServerLogoutAcpRegistry, "server.logoutAcpRegistry"),
        (ServerGetTraceDiagnostics, "server.getTraceDiagnostics"),
        (ServerGetProcessDiagnostics, "server.getProcessDiagnostics"),
        (ServerGetHostResources, "server.getHostResources"),
        (ServerGetProcessResourceHistory, "server.getProcessResourceHistory"),
        (ServerGetResourceTelemetryHistory, "server.getResourceTelemetryHistory"),
        (ServerRetryResourceTelemetry, "server.retryResourceTelemetry"),
        (ServerCheckOtlpEndpoint, "server.checkOtlpEndpoint"),
        (ServerListTextGeneratorModels, "server.listTextGeneratorModels"),
    ],
    g3: [
        (ServerPrepareTriage, "server.prepareTriage"),
        (ServerSignalProcess, "server.signalProcess"),
        (ServerReportClientActivity, "server.reportClientActivity"),
        (ServerGetUsageSummary, "server.getUsageSummary"),
        (ServerRefreshUsageRates, "server.refreshUsageRates"),
        (ScheduledTasksSubscribe, "scheduledTasks.subscribe"),
        (ScheduledTasksUpsert, "scheduledTasks.upsert"),
        (ScheduledTasksSetEnabled, "scheduledTasks.setEnabled"),
        (ScheduledTasksDelete, "scheduledTasks.delete"),
        (ScheduledTasksRunNow, "scheduledTasks.runNow"),
        (ScheduledTasksRotateWebhookToken, "scheduledTasks.rotateWebhookToken"),
        (SecretsAnswerRequest, "secrets.answerRequest"),
        (GuestShareCreate, "guestShare.create"),
        (ScheduledTasksListWebhookDeliveries, "scheduledTasks.listWebhookDeliveries"),
        (ScheduledTasksGetWebhookDelivery, "scheduledTasks.getWebhookDelivery"),
        (CloudGetRelayClientStatus, "cloud.getRelayClientStatus"),
        (CloudInstallRelayClient, "cloud.installRelayClient"),
        (PullRequestsList, "pullRequests.list"),
        (PullRequestsListStats, "pullRequests.listStats"),
        (PullRequestsFilterOptions, "pullRequests.filterOptions"),
        (PullRequestsSummary, "pullRequests.summary"),
        (PullRequestsRouting, "pullRequests.routing"),
        (PullRequestsRoutingIdentity, "pullRequests.routingIdentity"),
        (PullRequestsStack, "pullRequests.stack"),
        (PullRequestsLinkedThreads, "pullRequests.linkedThreads"),
        (PullRequestsDetail, "pullRequests.detail"),
        (PullRequestsPreview, "pullRequests.preview"),
        (PullRequestsChecks, "pullRequests.checks"),
        (PullRequestsActivity, "pullRequests.activity"),
        (PullRequestsReviewGuide, "pullRequests.reviewGuide"),
        (PullRequestsGenerateReviewGuide, "pullRequests.generateReviewGuide"),
        (PullRequestsThreadComments, "pullRequests.threadComments"),
        (PullRequestsDiffFileContents, "pullRequests.diffFileContents"),
        (PullRequestsFilesViewed, "pullRequests.filesViewed"),
        (PullRequestsChangeBreakdown, "pullRequests.changeBreakdown"),
        (PullRequestsSetFilesViewed, "pullRequests.setFilesViewed"),
        (PullRequestsRunAction, "pullRequests.runAction"),
        (PullRequestsUpdate, "pullRequests.update"),
        (PullRequestsComment, "pullRequests.comment"),
        (PullRequestsUpdateComment, "pullRequests.updateComment"),
        (PullRequestsSubmitReview, "pullRequests.submitReview"),
        (PullRequestsReplyToThread, "pullRequests.replyToThread"),
        (PullRequestsSetThreadResolution, "pullRequests.setThreadResolution"),
        (PullRequestsSetReaction, "pullRequests.setReaction"),
        (PullRequestsInvalidate, "pullRequests.invalidate"),
        (PullRequestsReportState, "pullRequests.reportState"),
        (PullRequestsSubscribeRefreshes, "pullRequests.subscribeRefreshes"),
        (PullRequestsSubscribeLinkedThreadRefreshes, "pullRequests.subscribeLinkedThreadRefreshes"),
        (PullRequestsReviewerCandidates, "pullRequests.reviewerCandidates"),
        (PullRequestsRequestReviewers, "pullRequests.requestReviewers"),
        (PullRequestsLabelCandidates, "pullRequests.labelCandidates"),
        (PullRequestsSetLabels, "pullRequests.setLabels"),
        (IssuesList, "issues.list"),
        (IssuesDetail, "issues.detail"),
        (IssuesComments, "issues.comments"),
        (IssuesTemplates, "issues.templates"),
        (IssuesCandidates, "issues.candidates"),
        (IssuesCreate, "issues.create"),
        (IssuesComment, "issues.comment"),
        (IssuesUpdate, "issues.update"),
    ],
    g4: [
        (IssuesSetState, "issues.setState"),
        (IssuesReact, "issues.react"),
        (IssuesLinkedThreads, "issues.linkedThreads"),
        (IssueTrackersList, "issueTrackers.list"),
        (IssueTrackersConnect, "issueTrackers.connect"),
        (IssueTrackersBindProject, "issueTrackers.bindProject"),
        (SourceControlLookupRepository, "sourceControl.lookupRepository"),
        (SourceControlCloneRepository, "sourceControl.cloneRepository"),
        (SourceControlPublishRepository, "sourceControl.publishRepository"),
        (SourceControlSignInStart, "sourceControl.signInStart"),
        (SourceControlSignInWait, "sourceControl.signInWait"),
        (SourceControlSignInCancel, "sourceControl.signInCancel"),
        (SourceControlAppAccess, "sourceControl.appAccess"),
        (SourceControlStarred, "sourceControl.starred"),
        (SourceControlStar, "sourceControl.star"),
        (ProjectCloneStart, "projectClone.start"),
        (ProjectCloneCancel, "projectClone.cancel"),
        (ProjectCloneRetry, "projectClone.retry"),
        (SubscribeProjectClones, "subscribeProjectClones"),
        (SubscribeVcsStatus, "subscribeVcsStatus"),
        (SubscribeVcsBranch, "subscribeVcsBranch"),
        (VcsRefreshBranch, "vcs.refreshBranch"),
        (SubscribeWorktreeSetup, "subscribeWorktreeSetup"),
        (WorktreeSetupCancel, "worktreeSetup.cancel"),
        (SubscribeTerminalMetadata, "subscribeTerminalMetadata"),
        (SubscribePreviewEvents, "subscribePreviewEvents"),
        (SubscribeDiscoveredLocalServers, "subscribeDiscoveredLocalServers"),
        (SubscribeDeviceState, "subscribeDeviceState"),
        (SubscribeServerConfig, "subscribeServerConfig"),
        (SubscribeServerLifecycle, "subscribeServerLifecycle"),
        (SubscribeAuthAccess, "subscribeAuthAccess"),
        (SubscribeResourceTelemetry, "subscribeResourceTelemetry"),
        (OrchestrationDispatchCommand, "orchestration.dispatchCommand"),
        (OrchestrationGetTurnDiff, "orchestration.getTurnDiff"),
        (OrchestrationGetFullThreadDiff, "orchestration.getFullThreadDiff"),
        (OrchestrationSearchThreads, "orchestration.searchThreads"),
        (OrchestrationSearchThread, "orchestration.searchThread"),
        (OrchestrationSearchThreadStream, "orchestration.searchThreadStream"),
        (OrchestrationGetArchivedShellSnapshot, "orchestration.getArchivedShellSnapshot"),
        (OrchestrationGetThreadFamily, "orchestration.getThreadFamily"),
        (OrchestrationGetThreadTitles, "orchestration.getThreadTitles"),
        (OrchestrationGetThreadLinkMetadata, "orchestration.getThreadLinkMetadata"),
        (OrchestrationGetWorktreeThreadIds, "orchestration.getWorktreeThreadIds"),
        (OrchestrationGetOlderSettledThreads, "orchestration.getOlderSettledThreads"),
        (OrchestrationGetThreadProjection, "orchestration.getThreadProjection"),
        (OrchestrationGetWorkflowScript, "orchestration.getWorkflowScript"),
        (OrchestrationGetNativeChildWork, "orchestration.getNativeChildWork"),
        (OrchestrationStopWorkflow, "orchestration.stopWorkflow"),
        (OrchestrationForceStopThread, "orchestration.forceStopThread"),
        (OrchestrationGetTurnItem, "orchestration.getTurnItem"),
        (OrchestrationLaunchThread, "orchestration.launchThread"),
        (OrchestrationSubscribeShell, "orchestration.subscribeShell"),
        (OrchestrationSubscribeThread, "orchestration.subscribeThread"),
        (OrchestrationSubscribeCommandOutput, "orchestration.subscribeCommandOutput"),
        (OrchestrationSubscribePromptSuggestion, "orchestration.subscribePromptSuggestion"),
        (WorkflowsSubscribe, "workflows.subscribe"),
        (WorkflowsCounts, "workflows.counts"),
        (WorkflowsGet, "workflows.get"),
        (WorkflowsAnswer, "workflows.answer"),
        (WorkflowsResume, "workflows.resume"),
        (WorkflowsCancel, "workflows.cancel"),
        (WorkflowsGetDefinition, "workflows.getDefinition"),
        (WorkflowsSaveDefinition, "workflows.saveDefinition"),
        (WorkflowsDeleteDefinition, "workflows.deleteDefinition"),
        (WorkflowBoardSubscribe, "workflowBoard.subscribe"),
        (WorkflowBoardGetCard, "workflowBoard.getCard"),
        (WorkflowBoardGetArtifact, "workflowBoard.getArtifact"),
        (WorkflowBoardCreateCard, "workflowBoard.createCard"),
        (WorkflowBoardUpdateSpec, "workflowBoard.updateSpec"),
        (WorkflowBoardApproveSpec, "workflowBoard.approveSpec"),
        (WorkflowBoardStartStage, "workflowBoard.startStage"),
        (WorkflowBoardExploreStep, "workflowBoard.exploreStep"),
        (WorkflowBoardChallengeStep, "workflowBoard.challengeStep"),
        (WorkflowBoardUpdatePlan, "workflowBoard.updatePlan"),
        (WorkflowBoardApprovePlan, "workflowBoard.approvePlan"),
        (WorkflowBoardReturnForRevision, "workflowBoard.returnForRevision"),
        (WorkflowBoardPauseCard, "workflowBoard.pauseCard"),
        (WorkflowBoardResumeCard, "workflowBoard.resumeCard"),
        (WorkflowBoardRetryStep, "workflowBoard.retryStep"),
        (WorkflowBoardProposeAmendment, "workflowBoard.proposeAmendment"),
        (WorkflowBoardResolveFinding, "workflowBoard.resolveFinding"),
        (WorkflowBoardRecordManualAcceptance, "workflowBoard.recordManualAcceptance"),
        (WorkflowBoardApproveDelivery, "workflowBoard.approveDelivery"),
        (WorkflowBoardGetCardDiff, "workflowBoard.getCardDiff"),
        (WorkflowBoardPreviewCliImport, "workflowBoard.previewCliImport"),
        (WorkflowBoardApplyCliImport, "workflowBoard.applyCliImport"),
        (AgentFeedbackAnswerRequest, "agentFeedback.answerRequest"),
    ],
);

// @effect-diagnostics nodeBuiltinImport:off -- Test fixture generator writes temporary mock provider scripts.
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";

export interface ParityFixtureContext {
  readonly cwd: string;
  readonly secondaryCwd: string;
  readonly projectId: string;
  readonly secondaryProjectId: string;
  readonly threadId: string;
  readonly itemId: string;
  readonly attachmentId: string;
  readonly environmentId: string;
  readonly taskId: string;
  readonly terminalId: string;
  readonly previewTabId: string;
  readonly serverUrl: string;
  readonly commandId: (prefix?: string) => string;
}

export interface RpcFixtureSpec {
  readonly payload: unknown;
  /** External CLI/auth/network/unconfigured RPCs that may return a schema-valid Exit.Failure. */
  readonly allowFailureTags?: ReadonlyArray<string>;
  /** True for streams that only emit on live events rather than immediately on subscribe. */
  readonly allowZeroChunks?: boolean;
  /** Skip generic dispatch when tested by an explicit ordered lifecycle step. */
  readonly deferLifecycle?: boolean;
}

interface MockProviderScripts {
  readonly claudeBinaryPath: string;
  readonly codexBinaryPath: string;
  readonly acpBinaryPath: string;
  readonly failBinaryPath: string;
}

function writeExecutableShim(
  dir: string,
  name: string,
  scriptPath: string,
  isWindows: boolean,
): string {
  if (isWindows) {
    const shimPath = NodePath.join(dir, `${name}.cmd`);
    NodeFS.writeFileSync(shimPath, `@echo off\r\n"${process.execPath}" "${scriptPath}" %*\r\n`);
    return shimPath;
  }
  const shimPath = NodePath.join(dir, name);
  NodeFS.writeFileSync(shimPath, `#!/bin/sh\nexec "${process.execPath}" "${scriptPath}" "$@"\n`);
  NodeFS.chmodSync(shimPath, 0o755);
  return shimPath;
}

export function writeMockProviderScripts(dir: string, isWindows: boolean): MockProviderScripts {
  NodeFS.mkdirSync(dir, { recursive: true });

  // Claude stream-json: emits partial stream_event deltas followed by the final assistant frame + usage.
  // Verifies deduplication (`saw_stream_text_delta`) and TurnTokenUsage emission.
  const claudeScriptPath = NodePath.join(dir, "mock-claude.mjs");
  NodeFS.writeFileSync(
    claudeScriptPath,
    `import readline from "node:readline";
process.stdin.resume();
console.log(JSON.stringify({ type: "system", subtype: "init", session_id: "claude-sess-1" }));
console.log(JSON.stringify({
  type: "stream_event",
  event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
}));
console.log(JSON.stringify({
  type: "stream_event",
  event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hello from Claude mock." } },
}));
console.log(JSON.stringify({
  type: "stream_event",
  event: { type: "content_block_stop", index: 0 },
}));
console.log(JSON.stringify({
  type: "assistant",
  message: {
    id: "msg-1",
    role: "assistant",
    content: [{ type: "text", text: "Hello from Claude mock." }],
    usage: { input_tokens: 12, output_tokens: 7 },
  },
}));
console.log(JSON.stringify({
  type: "result",
  subtype: "success",
  is_error: false,
  usage: { input_tokens: 12, output_tokens: 7 },
}));
process.exit(0);
`,
  );

  // Codex app-server JSON-RPC stdio mock.
  const codexScriptPath = NodePath.join(dir, "mock-codex.mjs");
  NodeFS.writeFileSync(
    codexScriptPath,
    `import readline from "node:readline";
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (raw) => {
  const line = raw.trim();
  if (!line) return;
  const msg = JSON.parse(line);
  if (msg.method === "initialize") {
    console.log(JSON.stringify({ id: msg.id, result: { userAgent: "mock-codex/1.0" } }));
    return;
  }
  if (msg.method === "thread/start") {
    console.log(JSON.stringify({ id: msg.id, result: { thread: { id: "codex-th-1" } } }));
    return;
  }
  if (msg.method === "turn/start") {
    console.log(JSON.stringify({ id: msg.id, result: { turn: { id: "codex-turn-1" } } }));
    console.log(JSON.stringify({
      method: "item/agentMessage/delta",
      params: { threadId: "codex-th-1", turnId: "codex-turn-1", itemId: "item-1", delta: "Hello from Codex mock." },
    }));
    console.log(JSON.stringify({
      method: "turn/completed",
      params: { threadId: "codex-th-1", turn: { id: "codex-turn-1", status: "completed" } },
    }));
    process.exit(0);
  }
});
`,
  );

  // ACP JSON-RPC stdio mock (antigravity / kiro / opencode / grok ACP protocol).
  const acpScriptPath = NodePath.join(dir, "mock-acp.mjs");
  NodeFS.writeFileSync(
    acpScriptPath,
    `import readline from "node:readline";
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (raw) => {
  const line = raw.trim();
  if (!line) return;
  const msg = JSON.parse(line);
  if (msg.method === "initialize") {
    console.log(JSON.stringify({
      jsonrpc: "2.0",
      id: msg.id,
      result: { protocolVersion: 1, agentCapabilities: { loadSession: false } },
    }));
    return;
  }
  if (msg.method === "session/new") {
    console.log(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result: { sessionId: "acp-sess-1" } }));
    return;
  }
  if (msg.method === "session/prompt") {
    console.log(JSON.stringify({
      jsonrpc: "2.0",
      method: "session/update",
      params: {
        sessionId: "acp-sess-1",
        update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Hello from ACP mock." } },
      },
    }));
    console.log(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result: { stopReason: "end_turn" } }));
    process.exit(0);
  }
});
`,
  );

  const failScriptPath = NodePath.join(dir, "mock-fail.mjs");
  NodeFS.writeFileSync(
    failScriptPath,
    `console.error("simulated provider failure");
process.exit(2);
`,
  );

  return {
    claudeBinaryPath: writeExecutableShim(dir, "mock-claude", claudeScriptPath, isWindows),
    codexBinaryPath: writeExecutableShim(dir, "mock-codex", codexScriptPath, isWindows),
    acpBinaryPath: writeExecutableShim(dir, "mock-acp", acpScriptPath, isWindows),
    failBinaryPath: writeExecutableShim(dir, "mock-fail", failScriptPath, isWindows),
  };
}

export function buildWsRpcFixtures(ctx: ParityFixtureContext): Record<string, RpcFixtureSpec> {
  const nowIso = "2026-01-01T00:00:00.000Z";
  const prBase = { projectId: ctx.projectId, repository: "org/repo", number: 1 };
  const issueBase = { projectId: ctx.projectId, tracker: "github", scope: "org/repo" };
  const dummyProfile = {
    registration: {
      clientId: "oaiapp_client1",
      subject: "sub-1",
      email: null,
      redirectUri: "http://127.0.0.1:1455/auth/callback",
      connectionLabel: "dev",
      sharingEnabled: false,
      idTokenHint: "hint",
    },
    credentials: {
      clientId: "oaiapp_client1",
      accessToken: "tok",
      refreshToken: null,
      idToken: "id-tok",
      issuer: "https://auth.openai.com",
      expiresAt: 1893456000000,
      earliestRefreshAt: null,
      scopes: [],
      subject: "sub-1",
      email: null,
    },
  };

  return {
    "issues.list": { payload: { state: "open" } },
    "issues.detail": {
      payload: { ...issueBase, key: "1" },
      allowFailureTags: ["IssueUnavailableError", "IssueOperationError"],
    },
    "issues.comments": {
      payload: { ...issueBase, key: "1", cursor: "cursor-1" },
    },
    "issues.templates": { payload: issueBase },
    "issues.candidates": { payload: issueBase },
    "issues.linkedThreads": {
      payload: { tracker: "github", host: "github.com", scope: "org/repo", key: "1" },
    },
    "issues.create": {
      payload: { ...issueBase, title: "Test issue", body: "Body", labels: [], assignees: [] },
      allowFailureTags: ["IssueUnavailableError", "IssueOperationError"],
    },
    "issues.comment": {
      payload: { ...issueBase, key: "1", body: "Comment" },
      allowFailureTags: ["IssueUnavailableError", "IssueOperationError"],
    },
    "issues.update": { payload: { ...issueBase, key: "1", title: "Updated" } },
    "issues.setState": { payload: { ...issueBase, key: "1", state: "closed" } },
    "issues.react": { payload: { ...issueBase, key: "1", content: "thumbs-up", reacted: true } },
    "issueTrackers.list": { payload: {} },
    "issueTrackers.connect": { payload: { tracker: "linear", token: null } },
    "issueTrackers.bindProject": {
      payload: { projectId: ctx.projectId, binding: { tracker: "linear", scope: "ENG" } },
    },
    "guestShare.create": {
      payload: { threadId: ctx.threadId },
      allowFailureTags: ["GuestShareCreateError"],
    },
    "server.probe": { payload: {} },
    "server.getConfig": { payload: {} },
    "server.refreshProviders": { payload: { modelCatalogOnly: true } },
    "server.updateProvider": {
      payload: { provider: "codex", targetVersion: "latest", instanceId: "codex" },
      allowFailureTags: ["ServerProviderUpdateError"],
    },
    "provider.consumeResetCredit": { payload: { instanceId: "claudeAgent" } },
    "provider.auth.start": {
      payload: {
        instanceId: "claudeAgent",
        methodId: "oauth",
        returnUrl: `${ctx.serverUrl}/cb`,
        callbackMode: "server",
      },
      allowFailureTags: ["ProviderAuthError"],
    },
    "provider.auth.complete": {
      payload: {
        instanceId: "claudeAgent",
        flowId: "flow-1",
        callbackUrl: `${ctx.serverUrl}/cb`,
      },
      allowFailureTags: ["ProviderAuthError"],
    },
    "provider.chatgpt.reconnect-profile": {
      payload: { instanceId: "codex", methodId: "chatgpt" },
      allowFailureTags: ["ProviderAuthError", "ProviderChatGptProfileError"],
    },
    "provider.chatgpt.import-profile": {
      payload: { instanceId: "codex", profile: dummyProfile },
      allowFailureTags: ["ProviderAuthError", "ProviderChatGptProfileError"],
    },
    "provider.chatgpt.handoff.subscribe": {
      payload: {
        instanceId: "codex",
        environmentId: ctx.environmentId,
        attemptId: "attempt-1",
        returnUrl: `${ctx.serverUrl}/cb`,
        profile: null,
      },
      allowZeroChunks: true,
    },
    "provider.auth.respond": {
      payload: {
        instanceId: "claudeAgent",
        flowId: "flow-1",
        interactionId: "int-1",
        response: { type: "credentials", values: {} },
      },
      allowFailureTags: ["ProviderAuthError"],
    },
    "provider.auth.cancel": { payload: { instanceId: "claudeAgent", flowId: "flow-1" } },
    "provider.auth.removeMethod": { payload: { instanceId: "claudeAgent", methodId: "oauth" } },
    "provider.auth.logout": { payload: { instanceId: "claudeAgent" } },
    "provider.auth.subscribe": { payload: { instanceId: "claudeAgent" }, allowZeroChunks: true },
    "provider.install.start": { payload: { instanceId: "claudeAgent" } },
    "provider.install.cancel": {
      payload: { instanceId: "claudeAgent", operationId: "op-1" },
    },
    "provider.install.subscribe": { payload: { instanceId: "claudeAgent" }, allowZeroChunks: true },
    "provider.install.remove": { payload: { instanceId: "claudeAgent" } },
    "server.updateServer": {
      payload: {
        targetVersion: "0.1.0",
        channel: "latest",
        targetDatabaseMigration: 0,
        stageOnly: true,
      },
      allowFailureTags: ["ServerSelfUpdateError"],
    },
    "server.getNpmRelease": {
      payload: { spec: "latest" },
      allowFailureTags: ["ServerSelfUpdateError"],
    },
    "server.updateServerWithProgress": {
      payload: {
        targetVersion: "0.1.0",
        channel: "latest",
        targetDatabaseMigration: 0,
        stageOnly: true,
      },
    },
    "server.commitDesktopUpdate": {
      payload: { requestId: "upd-1" },
      allowFailureTags: ["ServerSelfUpdateError"],
    },
    "server.upsertKeybinding": { payload: { key: "mod+shift+y", command: "chat.new" } },
    "server.removeKeybinding": { payload: { key: "mod+shift+y", command: "chat.new" } },
    "server.runStorageCleanup": { payload: {} },
    "server.getStorageCleanupReport": { payload: {} },
    "server.getDatabaseStorage": { payload: {} },
    "server.getSettings": { payload: {} },
    "server.reclaimDatabaseSpace": { payload: {} },
    "server.updateSettings": { payload: { patch: {} } },
    "peerLinks.list": { payload: {} },
    "peerLinks.link": {
      payload: {
        url: "https://127.0.0.1:9",
        alternateUrls: [],
        pairingCode: "000000",
        access: "read-only",
        expectedEnvironmentId: "env-missing",
        expectedLabel: "Missing",
      },
      allowFailureTags: ["PeerLinkError"],
    },
    "peerLinks.unlink": { payload: { environmentId: "env-missing" } },
    "peerLinks.answerRequest": {
      payload: {
        threadId: ctx.threadId,
        turnItemId: ctx.itemId,
        answer: { type: "decline" },
      },
    },
    "server.discoverSourceControl": { payload: {} },
    "server.checkSecretSource": {
      payload: { source: { provider: "keychain", reference: "demo" } },
    },
    "server.searchAcpRegistry": { payload: { query: "claude" } },
    "server.prepareAcpRegistryAgent": {
      payload: { agentId: "missing-agent" },
      allowFailureTags: ["AcpRegistryOperationError"],
    },
    "server.uninstallAcpRegistryManagedBinary": {
      payload: { agentId: "missing-agent" },
      allowFailureTags: ["AcpRegistryOperationError"],
    },
    "server.acceptAcpRegistryUrlAuth": {
      payload: { instanceId: "acpRegistry", elicitationId: "elicit-1" },
      allowFailureTags: ["AcpRegistryOperationError"],
    },
    "server.listAcpRegistrySessions": {
      payload: { instanceId: "acpRegistry", projectId: ctx.projectId, cursor: "" },
    },
    "server.importAcpRegistrySession": {
      payload: {
        instanceId: "acpRegistry",
        projectId: ctx.projectId,
        sessionId: "sess-1",
        title: "Demo",
        updatedAt: nowIso,
      },
      allowFailureTags: ["AcpRegistryOperationError"],
    },
    "server.deleteAcpRegistrySession": {
      payload: { instanceId: "acpRegistry", projectId: ctx.projectId, sessionId: "sess-1" },
    },
    "server.listAcpRegistryProviders": {
      payload: { instanceId: "acpRegistry", projectId: ctx.projectId },
    },
    "server.setAcpRegistryProvider": {
      payload: {
        instanceId: "acpRegistry",
        projectId: ctx.projectId,
        providerId: "p-1",
        apiType: "openai-responses",
        baseUrl: "http://127.0.0.1:9",
        headers: {},
      },
    },
    "server.disableAcpRegistryProvider": {
      payload: { instanceId: "acpRegistry", projectId: ctx.projectId, providerId: "p-1" },
    },
    "server.logoutAcpRegistry": { payload: { instanceId: "acpRegistry" } },
    "server.getTraceDiagnostics": { payload: {} },
    "server.getProcessDiagnostics": { payload: {} },
    "server.getHostResources": { payload: {} },
    "server.getProcessResourceHistory": { payload: { windowMs: 60000, bucketMs: 1000 } },
    "server.getResourceTelemetryHistory": { payload: { windowMs: 60000, bucketMs: 1000 } },
    "server.retryResourceTelemetry": { payload: {} },
    "server.checkOtlpEndpoint": { payload: { signal: "traces", url: "http://127.0.0.1:9" } },
    "server.listTextGeneratorModels": { payload: { id: "local-openai" } },
    "server.prepareTriage": {
      payload: {
        client: {
          label: "Test Browser",
          deviceType: "desktop",
          os: "Windows",
          osMajorVersion: 11,
          deviceModel: "PC",
          surface: "web",
          webDeployment: "server",
          browser: "Chromium",
          appVersion: "0.0.1",
        },
      },
    },
    "server.getUsageSummary": {
      payload: { sinceDay: "2026-01-01", untilDay: "2026-01-02", timeZone: "UTC" },
    },
    "server.refreshUsageRates": { payload: {} },
    "server.signalProcess": { payload: { pid: 999999, startTimeMs: 0, signal: "SIGINT" } },
    "scheduledTasks.subscribe": { payload: {} },
    "scheduledTasks.upsert": {
      payload: {
        id: ctx.taskId,
        title: "Parity Task",
        prompt: "Say hello",
        enabled: true,
        schedule: { type: "interval", everyMs: 60000 },
        projectId: ctx.projectId,
        workspaceStrategy: { type: "root" },
        modelSelection: { instanceId: "devReplay", model: "replay-default", options: [] },
        runtimeMode: "full-access",
        interactionMode: "default",
      },
    },
    "scheduledTasks.setEnabled": { payload: { id: ctx.taskId, enabled: false } },
    "scheduledTasks.rotateWebhookToken": { payload: { id: ctx.taskId } },
    "scheduledTasks.runNow": { payload: { id: ctx.taskId } },
    "scheduledTasks.listWebhookDeliveries": { payload: { id: ctx.taskId } },
    "scheduledTasks.getWebhookDelivery": {
      payload: { id: ctx.taskId, deliveryId: "missing-delivery" },
      allowFailureTags: ["ScheduledTaskError"],
    },
    "scheduledTasks.delete": { payload: { id: ctx.taskId } },
    "secrets.answerRequest": {
      payload: {
        threadId: ctx.threadId,
        turnItemId: ctx.itemId,
        answer: { type: "decline" },
      },
    },
    "server.reportClientActivity": {
      payload: {
        environmentId: ctx.environmentId,
        clientId: "client-parity",
        clientKind: "web",
        visible: true,
        focused: true,
        recentlyInteracted: true,
        appState: "active",
        lowPowerMode: "false",
        scopes: [{ type: "server-config" }],
        ttlMs: 30000,
        observedAt: nowIso,
      },
    },
    "cloud.getRelayClientStatus": { payload: {} },
    "cloud.installRelayClient": { payload: {} },
    "pullRequests.list": { payload: { state: "open" } },
    "pullRequests.listStats": { payload: { refs: [prBase] } },
    "pullRequests.filterOptions": { payload: { kind: "labels", projectId: ctx.projectId } },
    "pullRequests.summary": {
      payload: prBase,
      allowFailureTags: ["PullRequestUnavailableError", "PullRequestOperationError"],
    },
    "pullRequests.routing": { payload: prBase },
    "pullRequests.routingIdentity": { payload: { host: "github.com" } },
    "pullRequests.stack": { payload: prBase },
    "pullRequests.linkedThreads": { payload: prBase },
    "pullRequests.detail": {
      payload: prBase,
      allowFailureTags: ["PullRequestUnavailableError", "PullRequestOperationError"],
    },
    "pullRequests.preview": {
      payload: prBase,
      allowFailureTags: ["PullRequestUnavailableError", "PullRequestOperationError"],
    },
    "pullRequests.checks": { payload: prBase },
    "pullRequests.activity": { payload: prBase },
    "pullRequests.reviewGuide": { payload: prBase },
    "pullRequests.generateReviewGuide": {
      payload: prBase,
      allowFailureTags: ["PullRequestUnavailableError", "PullRequestOperationError"],
    },
    "pullRequests.threadComments": { payload: { ...prBase, threadId: "t-1", cursor: "cursor-1" } },
    "pullRequests.diffFileContents": {
      payload: { ...prBase, changeType: "change", oldPath: "README.md", newPath: "README.md" },
    },
    "pullRequests.filesViewed": { payload: prBase },
    "pullRequests.changeBreakdown": { payload: prBase },
    "pullRequests.setFilesViewed": {
      payload: { ...prBase, files: [{ path: "README.md", viewed: true }] },
    },
    "pullRequests.runAction": {
      payload: { ...prBase, action: "close" },
      allowFailureTags: ["PullRequestUnavailableError", "PullRequestOperationError"],
    },
    "pullRequests.update": {
      payload: { ...prBase, title: "Updated" },
      allowFailureTags: ["PullRequestUnavailableError", "PullRequestOperationError"],
    },
    "pullRequests.comment": {
      payload: { ...prBase, body: "LGTM" },
      allowFailureTags: ["PullRequestUnavailableError", "PullRequestOperationError"],
    },
    "pullRequests.updateComment": {
      payload: { ...prBase, commentId: "c-1", kind: "issue-comment", body: "Updated" },
    },
    "pullRequests.submitReview": {
      payload: { ...prBase, verdict: "comment", body: "LGTM", comments: [] },
    },
    "pullRequests.replyToThread": {
      payload: { ...prBase, threadId: "t-1", body: "Reply" },
      allowFailureTags: ["PullRequestUnavailableError", "PullRequestOperationError"],
    },
    "pullRequests.setThreadResolution": {
      payload: { ...prBase, threadId: "t-1", resolved: true },
    },
    "pullRequests.setReaction": {
      payload: { ...prBase, content: "thumbs-up", reacted: true },
    },
    "pullRequests.invalidate": { payload: {} },
    "pullRequests.reportState": { payload: { reference: prBase, state: "open" } },
    "pullRequests.subscribeRefreshes": { payload: {}, allowZeroChunks: true },
    "pullRequests.subscribeLinkedThreadRefreshes": { payload: {}, allowZeroChunks: true },
    "pullRequests.reviewerCandidates": { payload: prBase },
    "pullRequests.requestReviewers": {
      payload: { ...prBase, reviewers: [{ id: "alice", kind: "user" }], requested: true },
    },
    "pullRequests.labelCandidates": { payload: prBase },
    "pullRequests.setLabels": { payload: { ...prBase, labels: ["bug"], applied: true } },
    "sourceControl.lookupRepository": {
      payload: { provider: "github", repository: "octocat/Hello-World", cwd: ctx.cwd },
      allowFailureTags: ["SourceControlError"],
    },
    "sourceControl.cloneRepository": {
      payload: {
        provider: "github",
        repository: "missing/missing",
        destinationPath: NodePath.join(ctx.secondaryCwd, "clone-target"),
      },
      allowFailureTags: ["SourceControlRepositoryError", "SourceControlError"],
    },
    "sourceControl.publishRepository": {
      payload: {
        cwd: ctx.cwd,
        provider: "github",
        repository: "octocat/demo",
        visibility: "private",
      },
      allowFailureTags: ["SourceControlRepositoryError", "SourceControlError"],
    },
    "sourceControl.signInStart": {
      payload: { provider: "github", host: "github.com" },
      allowFailureTags: ["SourceControlSignInError", "SourceControlError"],
    },
    "sourceControl.signInWait": {
      payload: { provider: "github", host: "github.com" },
      allowFailureTags: ["SourceControlSignInError", "SourceControlError"],
    },
    "sourceControl.signInCancel": { payload: { provider: "github", host: "github.com" } },
    "sourceControl.appAccess": {
      payload: { provider: "github", host: "github.com", repository: "octocat/Hello-World" },
    },
    "sourceControl.starred": {
      payload: { provider: "github", host: "github.com", repository: "octocat/Hello-World" },
    },
    "sourceControl.star": {
      payload: { provider: "github", host: "github.com", repository: "octocat/Hello-World" },
    },
    "projectClone.start": {
      payload: {
        projectId: "proj-clone-parity",
        title: "Clone Parity",
        createdAt: nowIso,
        provider: "github",
        repository: "missing/missing",
        destinationPath: NodePath.join(ctx.secondaryCwd, "clone-target-2"),
      },
      allowFailureTags: ["SourceControlRepositoryError", "SourceControlError"],
    },
    "projectClone.cancel": { payload: { projectId: "proj-clone-parity" } },
    "projectClone.retry": {
      payload: { projectId: "proj-clone-parity" },
      allowFailureTags: ["SourceControlError"],
    },
    subscribeProjectClones: { payload: {}, allowZeroChunks: true },
    "projects.listEntries": { payload: { cwd: ctx.cwd } },
    "projects.listIgnoredFiles": { payload: { cwd: ctx.cwd, query: "" } },
    "projects.readFile": { payload: { cwd: ctx.cwd, relativePath: "README.md" } },
    "projects.searchContents": {
      payload: {
        cwd: ctx.cwd,
        query: "LegionCode",
        limit: 10,
        caseSensitive: false,
        wholeWord: false,
        useRegex: false,
      },
    },
    "projects.searchEntries": { payload: { cwd: ctx.cwd, query: "README", limit: 10 } },
    "projects.ensureScratch": { payload: {} },
    "projects.createNew": { payload: { name: "Parity New Proj" } },
    "projects.createFromThread": { payload: { threadId: ctx.threadId } },
    "projects.writeFile": {
      payload: { cwd: ctx.cwd, relativePath: "notes.txt", contents: "Parity write\n" },
    },
    "projects.mutate": {
      payload: {
        type: "project.create",
        commandId: ctx.commandId("proj-create"),
        projectId: ctx.secondaryProjectId,
        title: "Secondary Parity Project",
        workspaceRoot: ctx.secondaryCwd,
      },
    },
    "shell.openInEditor": {
      payload: { cwd: ctx.cwd, editor: "vscode" },
      allowFailureTags: ["ExternalLauncherCommandNotFoundError", "OpenInEditorError"],
    },
    "filesystem.browse": { payload: { partialPath: `${ctx.cwd}/` } },
    "filesystem.getMetadata": { payload: { paths: [NodePath.join(ctx.cwd, "README.md")] } },
    "agentSessions.scan": { payload: {} },
    "agentSessions.import": { payload: { projectId: ctx.projectId } },
    "assets.createUrl": {
      payload: {
        resource: { _tag: "workspace-file", threadId: ctx.threadId, path: "README.md" },
      },
    },
    "assets.persistChatAttachments": {
      payload: {
        threadId: ctx.threadId,
        messageId: "msg-att-1",
        attachments: [
          {
            id: ctx.attachmentId,
            type: "image",
            name: "dot.png",
            mimeType: "image/png",
            sizeBytes: 68,
            dataUrl:
              "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
          },
        ],
      },
    },
    "attachments.createUploadUrl": {
      payload: { type: "image", name: "shot.png", mimeType: "image/png", sizeBytes: 128 },
    },
    "attachments.delete": { payload: { attachmentId: ctx.attachmentId } },
    "mcpApps.callTool": {
      payload: { threadId: ctx.threadId, itemId: ctx.itemId, name: "tool", arguments: {} },
      allowFailureTags: ["McpAppRequestError"],
    },
    "mcpApps.toolInfo": {
      payload: { threadId: ctx.threadId, itemId: ctx.itemId, name: "tool" },
    },
    "mcpApps.readResource": {
      payload: { threadId: ctx.threadId, itemId: ctx.itemId, uri: "mcp://resource" },
      allowFailureTags: ["McpAppRequestError"],
    },
    "mcpApps.updateModelContext": {
      payload: {
        threadId: ctx.threadId,
        itemId: ctx.itemId,
        conversationThreadId: ctx.threadId,
      },
    },
    "provider.uploadFeedback": {
      payload: { threadId: ctx.threadId },
      allowFailureTags: ["ProviderFeedbackUploadError"],
    },
    subscribeVcsStatus: { payload: { cwd: ctx.cwd, includeRemote: false } },
    subscribeVcsBranch: { payload: { cwd: ctx.cwd } },
    "vcs.refreshBranch": { payload: { cwd: ctx.cwd } },
    subscribeWorktreeSetup: { payload: { threadId: ctx.threadId }, allowZeroChunks: true },
    "worktreeSetup.cancel": { payload: { threadId: ctx.threadId } },
    "vcs.pull": {
      payload: { cwd: ctx.cwd },
      allowFailureTags: ["GitCommandError"],
    },
    "vcs.refreshStatus": { payload: { cwd: ctx.cwd, localOnly: true } },
    "git.runStackedAction": {
      payload: {
        actionId: "act-1",
        cwd: ctx.cwd,
        action: "commit",
        commitMessage: "chore: parity",
      },
    },
    "git.resolvePullRequest": {
      payload: { cwd: ctx.cwd, reference: "#1" },
      allowFailureTags: ["GitCommandError", "GitManagerServiceError"],
    },
    "git.preparePullRequestThread": {
      payload: { cwd: ctx.cwd, reference: "#1", mode: "local" },
      allowFailureTags: ["GitCommandError", "GitManagerServiceError"],
    },
    "vcs.listRefs": { payload: { cwd: ctx.cwd } },
    "vcs.createWorktree": {
      payload: { cwd: ctx.cwd, refName: "parity-wt-branch", path: null },
    },
    "vcs.removeWorktree": {
      payload: { cwd: ctx.cwd, path: NodePath.join(ctx.cwd, ".missing-wt") },
      allowFailureTags: ["GitCommandError"],
    },
    "vcs.createRef": { payload: { cwd: ctx.cwd, refName: "parity-ref", switchRef: false } },
    "vcs.switchRef": { payload: { cwd: ctx.cwd, refName: "parity-ref" } },
    "vcs.init": { payload: { cwd: ctx.secondaryCwd, kind: "git" } },
    "review.getBranchCommits": { payload: { cwd: ctx.cwd } },
    "review.getDiffPreview": {
      payload: {
        cwd: ctx.cwd,
        ignoreWhitespace: false,
        sourceKind: "working-tree",
        trackedOnly: false,
        metadataOnly: false,
        file: { path: "README.md", previousPath: null, sourceKind: "working-tree" },
      },
    },
    "review.getDiffFileContents": {
      payload: {
        cwd: ctx.cwd,
        sourceKind: "working-tree",
        changeType: "change",
        baseRef: null,
        headRef: null,
        oldPath: "README.md",
        newPath: "README.md",
      },
    },
    "terminal.open": {
      payload: { threadId: ctx.threadId, terminalId: ctx.terminalId, cwd: ctx.cwd },
    },
    "terminal.attach": {
      payload: { threadId: ctx.threadId, terminalId: ctx.terminalId, cwd: ctx.cwd },
      allowZeroChunks: true,
    },
    "terminal.observe": {
      payload: { threadId: ctx.threadId, terminalId: ctx.terminalId },
      allowZeroChunks: true,
    },
    "terminal.write": {
      payload: {
        threadId: ctx.threadId,
        terminalId: ctx.terminalId,
        data: "echo hi\n",
        binary: false,
        reply: false,
      },
    },
    "terminal.resize": {
      payload: { threadId: ctx.threadId, terminalId: ctx.terminalId, cols: 80, rows: 24 },
    },
    "terminal.clear": { payload: { threadId: ctx.threadId, terminalId: ctx.terminalId } },
    "terminal.close": { payload: { threadId: ctx.threadId, terminalId: ctx.terminalId } },
    subscribeTerminalMetadata: { payload: {}, allowZeroChunks: true },
    "preview.open": {
      payload: { threadId: ctx.threadId, url: "http://127.0.0.1:3000" },
    },
    "preview.resize": {
      payload: {
        threadId: ctx.threadId,
        tabId: ctx.previewTabId,
        viewport: { _tag: "fill" },
      },
    },
    "preview.adjust": {
      payload: { threadId: ctx.threadId, tabId: ctx.previewTabId, colorScheme: "dark" },
    },
    "preview.list": { payload: { threadId: ctx.threadId } },
    "preview.close": { payload: { threadId: ctx.threadId, tabId: ctx.previewTabId } },
    "preview.clearProfile": { payload: { profileId: "default" } },
    "preview.reportProfiles": {
      payload: {
        profiles: [{ id: "default", name: "Default", kind: "persistent" }],
        defaultProfileId: "default",
      },
    },
    "preview.reportStatus": {
      payload: {
        threadId: ctx.threadId,
        tabId: ctx.previewTabId,
        navStatus: { _tag: "Idle" },
        canGoBack: false,
        canGoForward: false,
      },
    },
    subscribePreviewEvents: { payload: {}, allowZeroChunks: true },
    "preview.stopLocalServer": { payload: { pid: 999999, port: 3100 } },
    subscribeDiscoveredLocalServers: { payload: {}, allowZeroChunks: true },
    "device.configure": { payload: { onboardingCompleted: true } },
    "device.list": { payload: { inspectOnly: true } },
    "device.testHost": {
      payload: { kind: "ssh", id: "ssh-1", label: "SSH", target: "127.0.0.1" },
    },
    "device.easAccount": { payload: {}, allowFailureTags: ["DeviceEasAccountError"] },
    "device.simlockIdentity": { payload: {}, allowFailureTags: ["DeviceSimlockIdentityError"] },
    "device.open": {
      payload: { threadId: ctx.threadId, deviceId: "sim-1", platform: "ios" },
    },
    "device.close": { payload: { threadId: ctx.threadId, deviceId: "sim-1" } },
    "device.shutdown": { payload: { deviceId: "sim-1", platform: "ios" } },
    "device.detail": { payload: { deviceId: "sim-1" } },
    "device.action": {
      payload: { deviceId: "sim-1", type: "setAppearance", value: "dark" },
    },
    subscribeDeviceState: { payload: {} },
    subscribeServerConfig: { payload: {} },
    subscribeServerLifecycle: { payload: {} },
    subscribeAuthAccess: { payload: {} },
    subscribeResourceTelemetry: { payload: {} },
    "orchestration.dispatchCommand": {
      payload: {
        type: "thread.stop",
        commandId: ctx.commandId("stop"),
        threadId: ctx.threadId,
      },
    },
    "orchestration.getWorkflowScript": {
      payload: { threadId: ctx.threadId, scriptPath: "missing.mjs" },
      allowFailureTags: ["OrchestrationGetWorkflowScriptError"],
    },
    "orchestration.getNativeChildWork": { payload: { threadId: ctx.threadId } },
    "orchestration.stopWorkflow": {
      payload: { threadId: ctx.threadId, subagentId: "sub-missing" },
      allowFailureTags: ["OrchestrationV2StopWorkflowError"],
    },
    "orchestration.forceStopThread": {
      payload: { threadId: ctx.threadId, runId: "run-missing" },
      allowFailureTags: ["OrchestrationV2ForceStopThreadError"],
    },
    "orchestration.getTurnItem": {
      payload: { threadId: ctx.threadId, itemId: "item-missing" },
    },
    "orchestration.getTurnDiff": {
      payload: {
        fromTurnCount: 0,
        toTurnCount: 0,
        threadId: ctx.threadId,
        ignoreWhitespace: false,
      },
    },
    "orchestration.getFullThreadDiff": {
      payload: { threadId: ctx.threadId, toTurnCount: 0, ignoreWhitespace: false },
    },
    "orchestration.searchThreads": {
      payload: {
        query: "hello",
        limit: 10,
        olderSettledBefore: "",
        projectIds: [],
        environmentLabel: "",
        titlesOnly: false,
      },
    },
    "orchestration.searchThread": {
      payload: {
        threadId: ctx.threadId,
        theme: "dark",
        skills: [],
        query: "hello",
        index: 0,
        offset: 0,
        start: { entryId: "entry-1", occurrence: 0 },
      },
    },
    "orchestration.searchThreadStream": {
      payload: {
        threadId: ctx.threadId,
        theme: "dark",
        skills: [],
        query: "hello",
        index: 0,
        offset: 0,
        start: { entryId: "entry-1", occurrence: 0 },
      },
    },
    "orchestration.getArchivedShellSnapshot": { payload: {} },
    "orchestration.getThreadFamily": { payload: { threadId: ctx.threadId } },
    "orchestration.getThreadTitles": { payload: { threadIds: [ctx.threadId] } },
    "orchestration.getThreadLinkMetadata": { payload: { threadIds: [ctx.threadId] } },
    "orchestration.getWorktreeThreadIds": { payload: { worktreePath: ctx.cwd } },
    "orchestration.getOlderSettledThreads": {
      payload: { before: { settledAt: nowIso, threadId: "zzz" }, projectIds: [], limit: 10 },
    },
    "orchestration.getThreadProjection": { payload: { threadId: ctx.threadId } },
    "orchestration.launchThread": {
      payload: {
        commandId: ctx.commandId("launch-sweep"),
        projectId: ctx.projectId,
        title: "Sweep Thread",
        modelSelection: { instanceId: "devReplay", model: "replay-default", options: [] },
        runtimeMode: "full-access",
        interactionMode: "default",
        workspaceStrategy: { type: "root" },
      },
    },
    "orchestration.subscribeShell": {
      payload: {
        afterSequence: 0,
        httpSnapshotFallback: true,
        knownProjectIdentities: "",
        requestCompletionMarker: true,
        omitIdleSubagents: false,
        omitOlderSettled: false,
      },
    },
    "orchestration.subscribeThread": {
      payload: {
        threadId: ctx.threadId,
        afterSequence: 0,
        requestCompletionMarker: true,
        acceptBoundedSnapshot: true,
        acceptDeferredWork: false,
        acceptCompactTurnItems: false,
        acceptDedupedBodies: false,
        acceptCompactIds: false,
      },
    },
    "orchestration.subscribeCommandOutput": {
      payload: { threadId: ctx.threadId, itemId: ctx.itemId },
      allowZeroChunks: true,
    },
    "orchestration.subscribePromptSuggestion": {
      payload: { threadId: ctx.threadId },
      allowZeroChunks: true,
    },
  };
}

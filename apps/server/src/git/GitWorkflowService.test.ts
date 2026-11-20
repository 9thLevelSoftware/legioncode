import {
  ProjectId,
  ProviderInstanceId,
  SourceControlProviderError,
  SourceControlProviderKind,
  ThreadId,
  type GitRunStackedActionResult,
  type OrchestrationProjectShell,
  type OrchestrationV2ServerCommand,
} from "@t3tools/contracts";
import { assert, it } from "@effect/vitest";
import * as Effect from "effect/Effect";
import * as Layer from "effect/Layer";
import * as Ref from "effect/Ref";

import * as NodeCrypto from "@effect/platform-node/NodeCrypto";
import * as NodeServices from "@effect/platform-node/NodeServices";
import * as FileSystem from "effect/FileSystem";
import * as Path from "effect/Path";

import * as ServerConfig from "../config.ts";
import * as Orchestrator from "../orchestration-v2/Orchestrator.ts";
import * as ProjectionStore from "../orchestration-v2/ProjectionStore.ts";
import * as ProjectStore from "../orchestration-v2/ProjectStore.ts";
import { v2PullRequestThread } from "../orchestration-v2/testkit/pullRequestFixtures.ts";
import * as ProjectService from "../project/ProjectService.ts";
import * as ProjectSetupScriptRunner from "../project/ProjectSetupScriptRunner.ts";
import * as ProviderRegistry from "../provider/ProviderRegistry.ts";
import * as PullRequestService from "../pullRequest/PullRequestService.ts";
import * as ServerSettings from "../serverSettings.ts";
import * as SourceControlProviderRegistry from "../sourceControl/SourceControlProviderRegistry.ts";
import * as TextGeneration from "../textGeneration/TextGeneration.ts";
import * as VcsProcess from "../vcs/VcsProcess.ts";
import * as GitManager from "./GitManager.ts";
import * as GitWorkflowService from "./GitWorkflowService.ts";
import * as GitVcsDriver from "../vcs/GitVcsDriver.ts";
import * as VcsDriverRegistry from "../vcs/VcsDriverRegistry.ts";
import * as VcsProjectConfig from "../vcs/VcsProjectConfig.ts";

it.layer(Layer.mergeAll(NodeServices.layer, NodeCrypto.layer))("GitWorkflowService", (it) => {
  it.effect("finds a name among the refs of every namespace", () =>
    Effect.gen(function* () {
      const fileSystem = yield* FileSystem.FileSystem;
      const cwd = yield* fileSystem.makeTempDirectoryScoped({ prefix: "t3-ref-named-" });
      const workflow = yield* GitWorkflowService.GitWorkflowService;
      const driver = yield* GitVcsDriver.GitVcsDriver;
      for (const args of [
        ["init", "-b", "main"],
        ["-c", "user.email=t@t", "-c", "user.name=t", "commit", "--allow-empty", "-m", "init"],
        ["update-ref", "refs/remotes/upstream/feature/topic", "HEAD"],
        ["symbolic-ref", "refs/remotes/origin/alias", "refs/heads/main"],
        ["update-ref", "refs/mirror/origin/mirrored", "HEAD"],
      ]) {
        yield* driver.execute({ operation: "test.setupRepo", cwd, args });
      }
      const found = yield* Effect.forEach(
        ["main", "feature/topic", "alias", "mirrored", "t3/renamed", "feature/*"],
        (refName) => workflow.hasRefNamed({ cwd, refName }),
      );
      assert.deepEqual(found, [true, true, true, true, false, false]);
    }).pipe(
      Effect.scoped,
      Effect.provide(
        GitWorkflowService.layer.pipe(
          Layer.provide(
            Layer.mock(VcsDriverRegistry.VcsDriverRegistry)({
              resolve: () => Effect.succeed({ kind: "git" } as VcsDriverRegistry.VcsDriverHandle),
            }),
          ),
          Layer.provide(Layer.mock(GitManager.GitManager)({})),
          Layer.provideMerge(GitVcsDriver.layer),
          Layer.provideMerge(VcsProcess.layer),
          Layer.provideMerge(
            ServerConfig.layerTest(process.cwd(), { prefix: "t3-ref-named-" }).pipe(
              Layer.provide(Layer.mergeAll(NodeServices.layer, NodeCrypto.layer)),
            ),
          ),
          Layer.provideMerge(Layer.mergeAll(NodeServices.layer, NodeCrypto.layer)),
        ),
      ),
    ),
  );

  it.effect("reads no HEAD path from a removed worktree without spawning Git", () =>
    Effect.gen(function* () {
      const fileSystem = yield* FileSystem.FileSystem;
      const parent = yield* fileSystem.makeTempDirectoryScoped({ prefix: "t3-removed-wt-" });
      const removed = `${parent}/removed`;
      const workflow = yield* GitWorkflowService.GitWorkflowService;
      assert.strictEqual(yield* workflow.headPath(removed), null);
    }).pipe(
      Effect.scoped,
      Effect.provide(
        GitWorkflowService.layer.pipe(
          Layer.provide(Layer.mock(VcsDriverRegistry.VcsDriverRegistry)({})),
          Layer.provide(Layer.mock(GitManager.GitManager)({})),
          Layer.provide(
            Layer.mock(GitVcsDriver.GitVcsDriver)({
              execute: (input) => Effect.die(`spawned Git in ${input.cwd}`),
            }),
          ),
        ),
      ),
    ),
  );

  it.effect(
    "links a created pull request and refreshes pushed pull requests on runStackedAction success",
    () =>
      Effect.gen(function* () {
        const projectId = ProjectId.make("project-1");
        const threadId = ThreadId.make("thread-1");
        const project: OrchestrationProjectShell = {
          id: projectId,
          title: "Project",
          workspaceRoot: "/repo",
          defaultModelSelection: null,
          scripts: [],
          createdAt: "2026-08-01T00:00:00.000Z",
          updatedAt: "2026-08-01T00:00:00.000Z",
        };
        const thread = v2PullRequestThread({
          id: threadId,
          projectId,
          title: "Thread",
          modelSelection: { instanceId: ProviderInstanceId.make("codex"), model: "gpt-5" },
          runtimeMode: "full-access",
          interactionMode: "default",
          branch: null,
          worktreePath: null,
          pullRequests: [],
          createdAt: "2026-08-01T00:00:00.000Z",
          updatedAt: "2026-08-20T00:00:00.000Z",
          archivedAt: null,
          settledOverride: null,
          settledAt: null,
          latestUserMessageAt: "2026-08-20T00:00:00.000Z",
        });
        const commands = yield* Ref.make<ReadonlyArray<OrchestrationV2ServerCommand>>([]);
        const refreshed = yield* Ref.make<ReadonlyArray<readonly [string, boolean | undefined]>>(
          [],
        );
        const result: GitRunStackedActionResult = {
          action: "commit_push_pr",
          branch: { status: "skipped_not_requested" },
          commit: { status: "created", commitSha: "abc1234", subject: "Commit" },
          push: { status: "pushed", branch: "feature", upstreamBranch: "origin/feature" },
          pr: {
            status: "created",
            number: 42,
            url: "https://github.com/t3tools/t3code/pull/42",
            baseBranch: "main",
            headBranch: "feature",
            title: "Feature",
          },
          toast: { title: "Done", cta: { kind: "none" } },
        };
        const workflow = yield* Effect.gen(function* () {
          const service = yield* GitWorkflowService.GitWorkflowService;
          return yield* service.runStackedAction({
            actionId: "action-1",
            cwd: "/repo",
            action: "commit_push_pr",
            threadId,
          });
        }).pipe(
          Effect.scoped,
          Effect.provide(
            Layer.mergeAll(
              GitWorkflowService.layer.pipe(
                Layer.provide(
                  Layer.mock(VcsDriverRegistry.VcsDriverRegistry)({
                    resolve: () =>
                      Effect.succeed({
                        kind: "git",
                        repository: { rootPath: "/repo" },
                      } as VcsDriverRegistry.VcsDriverHandle),
                  }),
                ),
                Layer.provide(
                  Layer.mock(GitManager.GitManager)({
                    runStackedAction: () => Effect.succeed(result),
                  }),
                ),
                Layer.provide(Layer.mock(GitVcsDriver.GitVcsDriver)({})),
              ),
              Layer.mock(Orchestrator.OrchestratorV2)({
                getThreadShell: () => Effect.succeed(thread),
                dispatch: (command) =>
                  Ref.update(commands, (recorded) => [...recorded, command]).pipe(
                    Effect.as({ sequence: 1, storedEvents: [] }),
                  ),
              }),
              Layer.mock(ProjectService.ProjectService)({
                getShell: () => Effect.succeedSome(project),
              }),
              Layer.mock(ProjectStore.ProjectStoreV2)({
                listShells: () => Effect.succeed([project]),
              }),
              Layer.mock(PullRequestService.PullRequestService)({
                refreshAfterTurn: (id, options) =>
                  Ref.update(refreshed, (calls) => [...calls, [id, options?.pushed] as const]),
              }),
            ),
          ),
        );
        assert.deepStrictEqual(workflow, result);
        assert.deepStrictEqual(
          (yield* Ref.get(commands)).map((command) => ({
            type: command.type,
            ...(command.type === "thread.pull-request.link"
              ? { number: command.number, source: command.source }
              : {}),
          })),
          [{ type: "thread.pull-request.link", number: 42, source: "created" }],
        );
        assert.deepStrictEqual(yield* Ref.get(refreshed), [[projectId, true]]);
      }),
  );

  it.effect("refreshes pushed pull requests when push succeeds before create_pr fails", () =>
    Effect.gen(function* () {
      const fileSystem = yield* FileSystem.FileSystem;
      const path = yield* Path.Path;
      const driver = yield* GitVcsDriver.GitVcsDriver;
      const remoteDir = yield* fileSystem.makeTempDirectoryScoped({ prefix: "t3-wf-remote-" });
      const cwd = yield* fileSystem.makeTempDirectoryScoped({ prefix: "t3-wf-repo-" });
      for (const args of [
        ["init", "--bare", remoteDir],
        ["init", "-b", "main", cwd],
        ["-C", cwd, "config", "user.email", "t@t"],
        ["-C", cwd, "config", "user.name", "t"],
        ["-C", cwd, "remote", "add", "origin", remoteDir],
      ]) {
        yield* driver.execute({ operation: "test.setupRepo", cwd, args });
      }
      yield* fileSystem.writeFileString(path.join(cwd, "README.md"), "init\n");
      for (const args of [
        ["add", "README.md"],
        ["commit", "-m", "init"],
        ["push", "-u", "origin", "main"],
        ["checkout", "-b", "feature"],
      ]) {
        yield* driver.execute({ operation: "test.setupBranch", cwd, args });
      }
      yield* fileSystem.writeFileString(path.join(cwd, "change.txt"), "change\n");

      const projectId = ProjectId.make("project-1");
      const threadId = ThreadId.make("thread-1");
      const project: OrchestrationProjectShell = {
        id: projectId,
        title: "Project",
        workspaceRoot: cwd,
        defaultModelSelection: null,
        scripts: [],
        createdAt: "2026-08-01T00:00:00.000Z",
        updatedAt: "2026-08-01T00:00:00.000Z",
      };
      const thread = v2PullRequestThread({
        id: threadId,
        projectId,
        title: "Thread",
        modelSelection: { instanceId: ProviderInstanceId.make("codex"), model: "gpt-5" },
        runtimeMode: "full-access",
        interactionMode: "default",
        branch: "feature",
        worktreePath: null,
        pullRequests: [],
        createdAt: "2026-08-01T00:00:00.000Z",
        updatedAt: "2026-08-20T00:00:00.000Z",
        archivedAt: null,
        settledOverride: null,
        settledAt: null,
        latestUserMessageAt: "2026-08-20T00:00:00.000Z",
      });
      const commands = yield* Ref.make<ReadonlyArray<OrchestrationV2ServerCommand>>([]);
      const refreshed = yield* Ref.make<ReadonlyArray<readonly [string, boolean | undefined]>>([]);
      const prError = new SourceControlProviderError({
        provider: SourceControlProviderKind.make("github"),
        operation: "createChangeRequest",
        cwd,
        detail: "Host rejected PR creation",
      });

      const appLayer = Layer.mergeAll(
        Layer.mock(Orchestrator.OrchestratorV2)({
          getThreadShell: () => Effect.succeed(thread),
          dispatch: (command) =>
            Ref.update(commands, (recorded) => [...recorded, command]).pipe(
              Effect.as({ sequence: 1, storedEvents: [] }),
            ),
        }),
        Layer.mock(ProjectService.ProjectService)({
          getShell: () => Effect.succeedSome(project),
        }),
        Layer.mock(ProjectStore.ProjectStoreV2)({
          listShells: () => Effect.succeed([project]),
          findActiveByWorkspaceRoot: () => Effect.succeedNone,
        }),
        Layer.mock(PullRequestService.PullRequestService)({
          refreshAfterTurn: (id, options) =>
            Ref.update(refreshed, (calls) => [...calls, [id, options?.pushed] as const]),
        }),
      );
      const workflowLayer = GitWorkflowService.layer.pipe(
        Layer.provide(
          GitManager.layer.pipe(
            Layer.provide(
              Layer.mergeAll(
                appLayer,
                Layer.mock(ProjectionStore.ProjectionStoreV2)({
                  getThreadShell: () => Effect.succeed(thread),
                  getThreadProjection: () =>
                    Effect.succeed({ visibleTurnItems: [] } as unknown as Effect.Success<
                      ReturnType<
                        ProjectionStore.ProjectionStoreV2["Service"]["getThreadProjection"]
                      >
                    >),
                }),
                Layer.mock(SourceControlProviderRegistry.SourceControlProviderRegistry)({
                  resolveLink: () => undefined,
                  resolve: () =>
                    Effect.succeed({
                      kind: SourceControlProviderKind.make("github"),
                      listChangeRequests: () => Effect.succeed([]),
                      getDefaultBranch: () => Effect.succeed("main"),
                      createChangeRequest: () => Effect.fail(prError),
                    } as unknown as SourceControlProviderRegistry.SourceControlProviderRegistry["Service"] extends {
                      resolve: (
                        ...args: ReadonlyArray<never>
                      ) => Effect.Effect<infer P, unknown, unknown>;
                    }
                      ? P
                      : never),
                }),
                Layer.mock(TextGeneration.TextGeneration)({
                  generateCommitMessage: () =>
                    Effect.succeed({
                      subject: "Add change",
                      body: "",
                      branch: "feature",
                    }),
                  generatePrContent: () => Effect.succeed({ title: "Add change", body: "Body" }),
                }),
                Layer.mock(ProviderRegistry.ProviderRegistry)({
                  getProviders: Effect.succeed([]),
                }),
                Layer.mock(ProjectSetupScriptRunner.ProjectSetupScriptRunner)({}),
                ServerSettings.layerTest(),
              ),
            ),
          ),
        ),
        Layer.provide(VcsDriverRegistry.layer.pipe(Layer.provide(VcsProjectConfig.layer))),
      );

      const error = yield* Effect.gen(function* () {
        const workflow = yield* GitWorkflowService.GitWorkflowService;
        return yield* workflow
          .runStackedAction({
            actionId: "action-partial",
            cwd,
            action: "commit_push_pr",
            threadId,
            commitMessage: "Add change",
          })
          .pipe(Effect.flip);
      }).pipe(Effect.provide(Layer.mergeAll(workflowLayer, appLayer)));

      assert.strictEqual(error._tag, "SourceControlProviderError");
      assert.deepStrictEqual(yield* Ref.get(commands), []);
      assert.deepStrictEqual(yield* Ref.get(refreshed), [[projectId, true]]);
    }).pipe(
      Effect.scoped,
      Effect.provide(
        Layer.mergeAll(GitVcsDriver.layer, VcsProcess.layer).pipe(
          Layer.provideMerge(ServerConfig.layerTest(process.cwd(), { prefix: "t3-wf-partial-" })),
          Layer.provideMerge(Layer.mergeAll(NodeServices.layer, NodeCrypto.layer)),
        ),
      ),
    ),
  );
});

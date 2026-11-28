// @effect-diagnostics nodeBuiltinImport:off -- E2E tests spawn isolated Rust server, verify HTTP/WS auth, and read repo catalogs/docs.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodeHttp from "node:http";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeURL from "node:url";

import * as NodeWS from "@effect/platform-node/NodeWS";
import { assert, describe, it } from "@effect/vitest";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Effect from "effect/Effect";
import { fetch } from "undici";

import { resolveRustCliBinary } from "./rustCutover.ts";
import {
  fetchEnvironmentInfo,
  resolveWorkflowBoardRepoRoot,
  startWorkflowBoardE2eHarness,
  WorkflowBoardRpcClient,
  type WorkflowBoardE2eHarness,
} from "./workflowBoardE2eHarness.ts";

const EXPECTED_TARGET_LOCALES = ["de", "es", "fr", "ja", "ko", "pt-BR", "ru", "sv", "zh"] as const;

const LOCK_TIMESTAMP_ISO = "2026-10-22T02:30:00.000Z";

function resolveBinaryOrAssert(platform: NodeJS.Platform, env: NodeJS.ProcessEnv): string {
  const repoRoot = resolveWorkflowBoardRepoRoot();
  const binaryPath = resolveRustCliBinary({ repoRoot, platform, env });
  assert.isDefined(binaryPath, "Expected built rust/dist/bin/legioncode binary");
  if (binaryPath === undefined) {
    throw new Error("Expected built rust/dist/bin/legioncode binary");
  }
  return binaryPath;
}

async function withMockHttpServer(
  handler: NodeHttp.RequestListener,
  run: (origin: string) => Promise<void>,
): Promise<void> {
  const server = NodeHttp.createServer(handler);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  try {
    const addr = server.address();
    if (!addr || typeof addr === "string") {
      throw new Error("Expected numeric HTTP server port");
    }
    await run(`http://127.0.0.1:${addr.port}`);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

async function withMockWsClient(
  onConnection: (ws: NodeWS.WebSocket) => void,
  run: (client: WorkflowBoardRpcClient) => Promise<void>,
): Promise<void> {
  const wss = new NodeWS.WebSocketServer({ host: "127.0.0.1", port: 0 });
  await new Promise<void>((resolve) => wss.once("listening", () => resolve()));
  const addr = wss.address();
  if (!addr || typeof addr === "string") {
    wss.close();
    throw new Error("Expected numeric WebSocketServer port");
  }
  wss.on("connection", onConnection);
  const clientWs = new NodeWS.WebSocket(`ws://127.0.0.1:${addr.port}`);
  await new Promise<void>((resolve, reject) => {
    clientWs.once("open", () => resolve());
    clientWs.once("error", reject);
  });
  const client = new WorkflowBoardRpcClient(clientWs);
  try {
    await run(client);
  } finally {
    client.close();
    await new Promise<void>((resolve) => wss.close(() => resolve()));
  }
}

function sendRawWsUpgrade(
  origin: string,
  pathAndQuery: string,
  headers: Record<string, string> = {},
): Promise<{ readonly statusCode: number }> {
  return new Promise((resolve, reject) => {
    const target = new NodeURL.URL(pathAndQuery, origin);
    const req = NodeHttp.request({
      hostname: target.hostname,
      port: target.port,
      path: `${target.pathname}${target.search}`,
      method: "GET",
      headers: {
        Upgrade: "websocket",
        Connection: "Upgrade",
        "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
        "Sec-WebSocket-Version": "13",
        ...headers,
      },
    });
    req.on("upgrade", (_res, socket) => {
      socket.destroy();
      resolve({ statusCode: 101 });
    });
    req.on("response", (res) => {
      res.resume();
      res.on("end", () => resolve({ statusCode: res.statusCode ?? 0 }));
    });
    req.on("error", reject);
    req.end();
  });
}

async function ensureProjectId(
  harness: WorkflowBoardE2eHarness,
  binaryPath: string,
): Promise<string> {
  const snap = (await harness.rpc.callUnary("orchestration.v2.getShellSnapshot", {})) as {
    readonly projects?: ReadonlyArray<{ readonly id: string }> | undefined;
  };
  if (snap.projects && snap.projects.length > 0 && snap.projects[0]?.id) {
    return snap.projects[0].id;
  }
  const addResult = NodeChildProcess.spawnSync(
    binaryPath,
    [
      "project",
      "add",
      harness.workspaceDir,
      "--title",
      "Board E2E Project",
      "--base-dir",
      harness.homeDir,
    ],
    { encoding: "utf8" },
  );
  if (addResult.status !== 0) {
    throw new Error(`legioncode project add failed: ${addResult.stderr}`);
  }
  const after = (await harness.rpc.callUnary("orchestration.v2.getShellSnapshot", {})) as {
    readonly projects?: ReadonlyArray<{ readonly id: string }> | undefined;
  };
  const projectId = after.projects?.[0]?.id;
  if (!projectId) {
    throw new Error("Expected at least one project after project add");
  }
  return projectId;
}

function readCardField(obj: Record<string, unknown>, ...keys: ReadonlyArray<string>): unknown {
  for (const key of keys) {
    if (key in obj && obj[key] !== undefined) {
      return obj[key];
    }
  }
  const nested = obj.card;
  if (nested && typeof nested === "object" && !Array.isArray(nested)) {
    const record = nested as Record<string, unknown>;
    for (const key of keys) {
      if (key in record && record[key] !== undefined) {
        return record[key];
      }
    }
  }
  return undefined;
}

function seedLocalActiveCliLock(
  workspaceDir: string,
  kind: "engine-lock" | "live-spawn" | "resume-json",
  pid: number,
  hostPlatform: NodeJS.Platform,
): string {
  const cliDir = NodePath.join(workspaceDir, ".legion-cli");
  const runId = "synthetic-active-run";
  const host = NodeOS.hostname();
  let lockPath: string;
  let payload: Record<string, unknown>;
  if (kind === "engine-lock") {
    lockPath = NodePath.join(cliDir, "index", "engine.lock");
    payload = {
      pid,
      acquiredAt: LOCK_TIMESTAMP_ISO,
      hostname: host,
      platform: hostPlatform,
    };
  } else if (kind === "live-spawn") {
    lockPath = NodePath.join(cliDir, "cache", "live-spawn", `${runId}.json`);
    payload = {
      schemaVersion: "legion-cli-live-run/v1",
      runId,
      skillId: "workflow-execute",
      taskId: "TSK-0001",
      enginePid: pid,
      agentPid: null,
      startedAt: LOCK_TIMESTAMP_ISO,
      hostname: host,
      platform: hostPlatform,
    };
  } else {
    lockPath = NodePath.join(cliDir, "cache", "runs", runId, "resume.json");
    payload = {
      schemaVersion: "legion-cli-resume/v1",
      runId,
      skillId: "workflow-execute",
      taskId: "TSK-0001",
      pid,
      enginePid: null,
      startedAt: LOCK_TIMESTAMP_ISO,
    };
  }
  NodeFS.mkdirSync(NodePath.dirname(lockPath), { recursive: true });
  NodeFS.writeFileSync(lockPath, JSON.stringify(payload, null, 2), "utf8");
  return lockPath;
}

function assertDomainRejection(err: unknown, expectedPattern: RegExp, context: string): void {
  const msg = err instanceof Error ? err.message : String(err);
  assert.notInclude(msg, "UnsupportedRpcMethod", `${context}: unexpected UnsupportedRpcMethod`);
  assert.notInclude(msg, "ParseError", `${context}: unexpected ParseError`);
  assert.match(msg, expectedPattern, `${context}: unexpected error message: ${msg}`);
}

function extractIcuPlaceholders(input: string): ReadonlyArray<string> {
  const vars = new Set<string>();
  const walk = (text: string): void => {
    let i = 0;
    let inQuotes = false;
    while (i < text.length) {
      const ch = text[i];
      if (inQuotes) {
        if (ch === "'") {
          if (text[i + 1] === "'") {
            i += 2;
            continue;
          }
          inQuotes = false;
          i += 1;
          continue;
        }
        i += 1;
        continue;
      }
      if (ch === "'") {
        const next = text[i + 1];
        if (next === "'") {
          i += 2;
          continue;
        }
        if (next === "{" || next === "}" || next === "#") {
          inQuotes = true;
          i += 2;
          continue;
        }
        i += 1;
        continue;
      }
      if (ch === "{") {
        let j = i + 1;
        while (j < text.length && text[j] !== "," && text[j] !== "}") {
          j += 1;
        }
        const name = text.slice(i + 1, j).trim();
        if (name && /^[A-Za-z0-9_]+$/u.test(name)) {
          vars.add(name);
        }
        if (text[j] === ",") {
          let depth = 1;
          let k = j + 1;
          let nestedQuotes = false;
          while (k < text.length && depth > 0) {
            const c = text[k];
            if (nestedQuotes) {
              if (c === "'") {
                if (text[k + 1] === "'") {
                  k += 2;
                  continue;
                }
                nestedQuotes = false;
              }
              k += 1;
              continue;
            }
            if (c === "'") {
              const next = text[k + 1];
              if (next === "'") {
                k += 2;
                continue;
              }
              if (next === "{" || next === "}" || next === "#") {
                nestedQuotes = true;
                k += 2;
                continue;
              }
              k += 1;
              continue;
            }
            if (c === "{") {
              if (depth === 1) {
                let branchDepth = 1;
                const branchStart = k + 1;
                let branchEnd = branchStart;
                let branchQuotes = false;
                while (branchEnd < text.length && branchDepth > 0) {
                  const bc = text[branchEnd];
                  if (branchQuotes) {
                    if (bc === "'") {
                      if (text[branchEnd + 1] === "'") {
                        branchEnd += 2;
                        continue;
                      }
                      branchQuotes = false;
                    }
                    branchEnd += 1;
                    continue;
                  }
                  if (bc === "'") {
                    const next = text[branchEnd + 1];
                    if (next === "'") {
                      branchEnd += 2;
                      continue;
                    }
                    if (next === "{" || next === "}" || next === "#") {
                      branchQuotes = true;
                      branchEnd += 2;
                      continue;
                    }
                    branchEnd += 1;
                    continue;
                  }
                  if (bc === "{") branchDepth += 1;
                  else if (bc === "}") branchDepth -= 1;
                  branchEnd += 1;
                }
                walk(text.slice(branchStart, branchEnd - 1));
                k = branchEnd;
                continue;
              }
              depth += 1;
            } else if (c === "}") {
              depth -= 1;
            }
            k += 1;
          }
          i = k;
          continue;
        }
        i = j + 1;
        continue;
      }
      i += 1;
    }
  };
  walk(input);
  return [...vars].sort();
}

function snapshotTreeDigests(dir: string): Map<string, string> {
  const files = new Map<string, string>();
  const walk = (current: string): void => {
    for (const entry of NodeFS.readdirSync(current, { withFileTypes: true })) {
      const abs = NodePath.join(current, entry.name);
      if (entry.isDirectory()) {
        walk(abs);
      } else if (entry.isFile()) {
        const rel = NodePath.relative(dir, abs).replaceAll("\\", "/");
        files.set(rel, NodeFS.readFileSync(abs, "utf8"));
      }
    }
  };
  walk(dir);
  return files;
}

describe("F22 typed contracts, capabilities & WS auth (server TS)", () => {
  it.live(
    "[F22-T1-01] advertises environment capabilities and decodes legacy and board-capable descriptors cleanly",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            requireBoardCapability: true,
          });
          assert.strictEqual(harness.environment.descriptor.orchestrationProtocolVersion, 2);
          assert.isTrue(
            harness.environment.projectWorkflowBoard,
            "Expected capabilities.projectWorkflowBoard === true",
          );

          const baseEnvPayload = {
            environmentId: "env-compat-probe",
            label: "compat-probe",
            platform: { os: "linux", arch: "x64" },
            serverVersion: "0.1.0-compat",
            orchestrationProtocolVersion: 2,
          };

          await withMockHttpServer(
            (req, res) => {
              if (req.url === "/.well-known/legioncode/environment") {
                res.writeHead(200, { "content-type": "application/json" });
                res.end(
                  JSON.stringify({ ...baseEnvPayload, capabilities: { repositoryIdentity: true } }),
                );
                return;
              }
              res.writeHead(404).end();
            },
            async (mockOrigin) => {
              const legacy = await fetchEnvironmentInfo(mockOrigin);
              assert.isFalse(legacy.projectWorkflowBoard);
            },
          );

          await withMockHttpServer(
            (req, res) => {
              if (req.url === "/.well-known/legioncode/environment") {
                res.writeHead(200, { "content-type": "application/json" });
                res.end(
                  JSON.stringify({
                    ...baseEnvPayload,
                    capabilities: { repositoryIdentity: true, projectWorkflowBoard: false },
                  }),
                );
                return;
              }
              res.writeHead(404).end();
            },
            async (mockOrigin) => {
              const disabled = await fetchEnvironmentInfo(mockOrigin);
              assert.isFalse(disabled.projectWorkflowBoard);
            },
          );

          await withMockHttpServer(
            (req, res) => {
              if (req.url === "/.well-known/legioncode/environment") {
                res.writeHead(200, { "content-type": "application/json" });
                res.end(
                  JSON.stringify({
                    ...baseEnvPayload,
                    capabilities: { repositoryIdentity: true, projectWorkflowBoard: true },
                  }),
                );
                return;
              }
              res.writeHead(404).end();
            },
            async (mockOrigin) => {
              const enabled = await fetchEnvironmentInfo(mockOrigin);
              assert.isTrue(enabled.projectWorkflowBoard);
            },
          );
        });
      }),
  );

  it.live(
    "[F22-T1-02] streams compact board summaries via workflowBoard.subscribe and fetches artifacts and diffs on demand",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
            requireBoardCapability: true,
          });
          assert.isTrue(
            harness.environment.projectWorkflowBoard,
            "Expected capabilities.projectWorkflowBoard === true",
          );

          const projectId = await ensureProjectId(harness, binaryPath);
          const sub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId });
          await sub.nextChunk();

          const created = (await harness.rpc.callUnary("workflowBoard.createCard", {
            projectId,
            title: "Compact summary card",
          })) as Record<string, unknown>;
          const cardId = String(readCardField(created, "cardId", "id"));
          const rev1 = Number(readCardField(created, "revision") ?? 1);

          const specSentinel = "SPEC_BODY_SENTINEL_ONLY_ON_DEMAND_48291";
          await harness.rpc.callUnary("workflowBoard.updateSpec", {
            cardId,
            expectedRevision: rev1,
            brief: specSentinel,
            acceptanceCriteria: [{ id: "AC-1", text: "Verify compact summary." }],
          });

          const updatedChunk = await sub.nextChunk((chunk) =>
            JSON.stringify(chunk).includes(cardId),
          );
          const summaryJson = JSON.stringify(updatedChunk);
          assert.include(summaryJson, cardId);
          assert.notInclude(summaryJson, specSentinel);

          const detail = await harness.rpc.callUnary("workflowBoard.getCard", { cardId });
          assert.include(JSON.stringify(detail), cardId);

          const artifact = await harness.rpc.callUnary("workflowBoard.getArtifact", {
            cardId,
            kind: "spec",
          });
          assert.include(JSON.stringify(artifact), specSentinel);

          const diff = await harness.rpc.callUnary("workflowBoard.getCardDiff", { cardId });
          assert.isDefined(diff);
          await sub.interrupt();
        });
      }),
  );

  it.live(
    "[F22-T1-03] enforces revision-checked mutations and preserves updated revision across server restart",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
            requireBoardCapability: true,
          });
          assert.isTrue(
            harness.environment.projectWorkflowBoard,
            "Expected capabilities.projectWorkflowBoard === true",
          );

          const projectId = await ensureProjectId(harness, binaryPath);
          const created = (await harness.rpc.callUnary("workflowBoard.createCard", {
            projectId,
            title: "Revision continuity card",
          })) as Record<string, unknown>;
          const cardId = String(readCardField(created, "cardId", "id"));
          const rev1 = Number(readCardField(created, "revision") ?? 1);

          const updated = (await harness.rpc.callUnary("workflowBoard.updateSpec", {
            cardId,
            expectedRevision: rev1,
            brief: "Updated revision sentinel.",
          })) as Record<string, unknown>;
          const rev2 = Number(readCardField(updated, "revision"));
          assert.isAbove(rev2, rev1);

          await harness.restart();

          const reloaded = (await harness.rpc.callUnary("workflowBoard.getCard", {
            cardId,
          })) as Record<string, unknown>;
          assert.strictEqual(Number(readCardField(reloaded, "revision")), rev2);
          assert.include(JSON.stringify(reloaded), "Updated revision sentinel.");
        });
      }),
  );

  it.live(
    "[F22-T2-01] rejects stale, zero, and ahead expectedRevision values without clobbering state",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
          });
          assert.isTrue(harness.environment.projectWorkflowBoard);

          const projectId = await ensureProjectId(harness, binaryPath);
          const created = (await harness.rpc.callUnary("workflowBoard.createCard", {
            projectId,
            title: "Revision conflict boundary card",
          })) as Record<string, unknown>;
          const cardId = String(readCardField(created, "cardId", "id"));
          const rev1 = Number(readCardField(created, "revision") ?? 1);

          const afterUpdate = (await harness.rpc.callUnary("workflowBoard.updateSpec", {
            cardId,
            expectedRevision: rev1,
            brief: "Canonical Revision 2",
          })) as Record<string, unknown>;
          const rev2 = Number(readCardField(afterUpdate, "revision"));

          for (const badRevision of [rev1, 0, rev2 + 99]) {
            let caughtErr: unknown;
            let rejected = false;
            try {
              await harness.rpc.callUnary("workflowBoard.updateSpec", {
                cardId,
                expectedRevision: badRevision,
                brief: `Overwrite attempt ${badRevision}`,
              });
            } catch (err) {
              rejected = true;
              caughtErr = err;
            }
            assert.isTrue(rejected, `expected revision ${badRevision} to be rejected`);
            if (badRevision === 0) {
              assert.include(String(caughtErr), "ParseError");
              assert.match(String(caughtErr), /revision.*(?:>= 1|greater than zero)/i);
            } else {
              assertDomainRejection(
                caughtErr,
                /revision_conflict/,
                `[F22-T2-01] badRevision=${badRevision}`,
              );
            }
          }

          const intact = (await harness.rpc.callUnary("workflowBoard.getCard", {
            cardId,
          })) as Record<string, unknown>;
          assert.strictEqual(Number(readCardField(intact, "revision")), rev2);

          const finalUpdate = (await harness.rpc.callUnary("workflowBoard.updateSpec", {
            cardId,
            expectedRevision: rev2,
            brief: "Valid Revision 3",
          })) as Record<string, unknown>;
          assert.isAbove(Number(readCardField(finalUpdate, "revision")), rev2);
        });
      }),
  );

  it.live(
    "[F22-T2-02] handles unsupported workflowBoard RPCs cleanly without poisoning WS multiplexer",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
          });

          if (!harness.environment.projectWorkflowBoard) {
            let unaryErr = "";
            try {
              await harness.rpc.callUnary("workflowBoard.getCard", { cardId: "nonexistent" });
            } catch (err) {
              unaryErr = err instanceof Error ? err.message : String(err);
            }
            assert.include(unaryErr, "UnsupportedRpcMethod");

            const sub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId: "p1" });
            let subErr = "";
            try {
              await sub.nextChunk();
            } catch (err) {
              subErr = err instanceof Error ? err.message : String(err);
            }
            assert.include(subErr, "UnsupportedRpcMethod");

            const probe = (await harness.rpc.callUnary("server.probe", {})) as {
              readonly serverTime?: string | undefined;
            };
            assert.isString(probe.serverTime);
            return;
          }

          await withMockWsClient(
            (ws) => {
              ws.on("message", (raw) => {
                const msg = JSON.parse(raw.toString("utf8")) as {
                  _tag: string;
                  id?: string;
                  tag?: string;
                };
                if (msg._tag !== "Request") return;
                if (msg.tag === "server.probe") {
                  ws.send(
                    JSON.stringify({
                      _tag: "Exit",
                      requestId: msg.id,
                      exit: { _tag: "Success", value: { serverTime: LOCK_TIMESTAMP_ISO } },
                    }),
                  );
                  return;
                }
                ws.send(
                  JSON.stringify({
                    _tag: "Exit",
                    requestId: msg.id,
                    exit: {
                      _tag: "Failure",
                      cause: [
                        { _tag: "Fail", error: { _tag: "UnsupportedRpcMethod", method: msg.tag } },
                      ],
                    },
                  }),
                );
              });
            },
            async (legacyClient) => {
              let errText = "";
              try {
                await legacyClient.callUnary("workflowBoard.getCard", { cardId: "c1" });
              } catch (err) {
                errText = err instanceof Error ? err.message : String(err);
              }
              assert.include(errText, "UnsupportedRpcMethod");
              const probe = (await legacyClient.callUnary("server.probe", {})) as {
                readonly serverTime?: string | undefined;
              };
              assert.strictEqual(probe.serverTime, LOCK_TIMESTAMP_ISO);
            },
          );
        });
      }),
  );

  it.live(
    "[F22-T2-03] enforces single-use wsTicket, protocol version, mcp-client rejection, and strict credential precedence",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
          });

          const ticketRes = await fetch(`${harness.origin}/api/auth/websocket-ticket/rpc`, {
            method: "POST",
            headers: { cookie: harness.cookiePair },
          });
          assert.strictEqual(ticketRes.status, 200);
          const ticketBody = (await ticketRes.json()) as { readonly ticket: string };
          const encodedTicket = encodeURIComponent(ticketBody.ticket);

          const firstUpgrade = await sendRawWsUpgrade(
            harness.origin,
            `/ws?orchestrationProtocol=2&wsTicket=${encodedTicket}`,
          );
          assert.strictEqual(firstUpgrade.statusCode, 101);

          const replayUpgrade = await sendRawWsUpgrade(
            harness.origin,
            `/ws?orchestrationProtocol=2&wsTicket=${encodedTicket}`,
          );
          assert.strictEqual(replayUpgrade.statusCode, 401);

          const badProtocol = await sendRawWsUpgrade(harness.origin, "/ws", {
            cookie: harness.cookiePair,
          });
          assert.strictEqual(badProtocol.statusCode, 426);

          const badBearerWithValidCookie = await sendRawWsUpgrade(
            harness.origin,
            "/ws?orchestrationProtocol=2",
            {
              authorization: "Bearer invalid-e2e-token",
              cookie: harness.cookiePair,
            },
          );
          assert.strictEqual(badBearerWithValidCookie.statusCode, 401);

          const mcpSessionRes = await fetch(`${harness.origin}/api/auth/sessions`, {
            method: "POST",
            headers: {
              cookie: harness.cookiePair,
              "content-type": "application/json",
            },
            body: JSON.stringify({
              subject: "mcp-client",
              label: "e2e-mcp-client",
              scopes: ["orchestration:read"],
            }),
          });
          assert.strictEqual(mcpSessionRes.status, 201);
          const mcpSession = (await mcpSessionRes.json()) as { readonly token: string };

          const mcpWsUpgrade = await sendRawWsUpgrade(
            harness.origin,
            `/ws?orchestrationProtocol=2&wsTicket=${encodeURIComponent(mcpSession.token)}`,
          );
          assert.strictEqual(mcpWsUpgrade.statusCode, 401);
        });
      }),
  );
});

describe("F23 multi-surface i18n, docs, evals & dist parity (server TS)", () => {
  it.live(
    "[F23-T1-05] enforces 10-locale catalog key parity across apps/web, apps/mobile, and apps/desktop",
    () =>
      Effect.sync(() => {
        const repoRoot = resolveWorkflowBoardRepoRoot();
        const apps = ["apps/web", "apps/mobile", "apps/desktop"] as const;
        const expectedSet = [...EXPECTED_TARGET_LOCALES].sort();

        for (const appRel of apps) {
          const appDir = NodePath.join(repoRoot, appRel);
          const gtConfig = JSON.parse(
            NodeFS.readFileSync(NodePath.join(appDir, "gt.config.json"), "utf8"),
          ) as {
            readonly defaultLocale: string;
            readonly locales: ReadonlyArray<string>;
            readonly files: { readonly gt: { readonly output: string } };
          };
          assert.strictEqual(gtConfig.defaultLocale, "en", `${appRel} defaultLocale`);
          assert.deepStrictEqual([...gtConfig.locales].sort(), expectedSet, `${appRel} locales`);

          const catalogDir = NodePath.join(appDir, NodePath.dirname(gtConfig.files.gt.output));
          const enCatalog = JSON.parse(
            NodeFS.readFileSync(NodePath.join(catalogDir, "en.json"), "utf8"),
          ) as Record<string, unknown>;
          const enKeys = new Set(Object.keys(enCatalog));
          assert.isTrue(enKeys.size > 0, `${appRel} en.json should not be empty`);

          for (const locale of gtConfig.locales) {
            const localeCatalog = JSON.parse(
              NodeFS.readFileSync(NodePath.join(catalogDir, `${locale}.json`), "utf8"),
            ) as Record<string, unknown>;
            const localeKeys = new Set(Object.keys(localeCatalog));
            const missing = [...enKeys].filter((k) => !localeKeys.has(k));
            const extra = [...localeKeys].filter((k) => !enKeys.has(k));
            assert.deepStrictEqual(missing, [], `${appRel} ${locale}.json missing keys`);
            assert.deepStrictEqual(extra, [], `${appRel} ${locale}.json extra keys`);
          }
        }

        const i18nCheckSource = NodeFS.readFileSync(
          NodePath.join(repoRoot, "scripts", "i18n-check.ts"),
          "utf8",
        );
        for (const appRel of apps) {
          assert.include(i18nCheckSource, `"${appRel}"`);
        }
      }),
  );

  it.live(
    "[F23-T1-06] checks evals and build-dist web bundle parity, plus docs/user/workflows.md and workflow evals when present",
    () =>
      Effect.sync(() => {
        const repoRoot = resolveWorkflowBoardRepoRoot();
        const evalsReadme = NodeFS.readFileSync(
          NodePath.join(repoRoot, "evals", "README.md"),
          "utf8",
        );
        assert.include(evalsReadme, "mcp-tool-use");
        const evalsPkg = JSON.parse(
          NodeFS.readFileSync(NodePath.join(repoRoot, "evals", "package.json"), "utf8"),
        ) as { readonly scripts: Record<string, string> };
        assert.isString(evalsPkg.scripts.test);

        const buildDistSrc = NodeFS.readFileSync(
          NodePath.join(repoRoot, "rust", "scripts", "build-dist.mjs"),
          "utf8",
        );
        assert.include(buildDistSrc, 'stageWebBundle(webDistDir, path.join(outDir, "web-app")');
        assert.include(
          buildDistSrc,
          'stageWebBundle(webDistDir, path.join(outDir, "bin", "client")',
        );
        assert.include(
          buildDistSrc,
          'stageWebBundle(webDistDir, path.join(outDir, "desktop", "web-app")',
        );

        const webDistIndex = NodePath.join(repoRoot, "apps", "web", "dist", "index.html");
        if (NodeFS.existsSync(webDistIndex)) {
          assert.isTrue(NodeFS.readFileSync(webDistIndex, "utf8").includes("<html"));
        }

        const workflowsDocPath = NodePath.join(repoRoot, "docs", "user", "workflows.md");
        const evalsRoot = NodePath.join(repoRoot, "evals");
        const evalDirs = NodeFS.readdirSync(evalsRoot, {
          withFileTypes: true,
        })
          .filter((entry) => entry.isDirectory() && entry.name.includes("workflow"))
          .map((entry) => entry.name);
        const mcpCasesDir = NodePath.join(evalsRoot, "mcp-tool-use", "cases");
        const mcpWorkflowCases = NodeFS.existsSync(mcpCasesDir)
          ? NodeFS.readdirSync(mcpCasesDir).filter((fileName) => {
              if (!fileName.endsWith(".ndjson")) return false;
              if (fileName.includes("workflow")) return true;
              const raw = NodeFS.readFileSync(NodePath.join(mcpCasesDir, fileName), "utf8");
              return /workflowBoard|workflow_board/i.test(raw);
            })
          : [];

        assert.isTrue(
          NodeFS.existsSync(workflowsDocPath),
          "Expected docs/user/workflows.md to exist",
        );
        assert.isTrue(
          evalDirs.length > 0 || mcpWorkflowCases.length > 0,
          "Expected workflow evals to exist under evals/",
        );

        const docText = NodeFS.readFileSync(workflowsDocPath, "utf8");
        for (const stage of ["Spec", "Plan", "Execute", "Ship"]) {
          assert.include(docText, stage);
        }
        assert.notInclude(docText, "orchestration-v2");
        assert.notInclude(docText, "statev2.sqlite");

        for (const dirName of evalDirs) {
          const suiteDir = NodePath.join(evalsRoot, dirName);
          assert.isTrue(NodeFS.existsSync(NodePath.join(suiteDir, "README.md")));
          assert.isTrue(NodeFS.existsSync(NodePath.join(suiteDir, "run.ts")));
          assert.isTrue(NodeFS.existsSync(NodePath.join(suiteDir, "run.test.ts")));
          const hasCases =
            NodeFS.existsSync(NodePath.join(suiteDir, "cases.ts")) ||
            NodeFS.existsSync(NodePath.join(suiteDir, "cases"));
          assert.isTrue(hasCases);
          assert.isString(evalsPkg.scripts[dirName]);
        }

        for (const caseFile of mcpWorkflowCases) {
          const lines = NodeFS.readFileSync(NodePath.join(mcpCasesDir, caseFile), "utf8")
            .split(/\r?\n/u)
            .map((line) => line.trim())
            .filter(Boolean);
          assert.isTrue(lines.length > 0, `${caseFile} should contain eval cases`);
          for (const line of lines) {
            assert.isObject(JSON.parse(line));
          }
        }
      }),
  );

  it.live(
    "[F23-T2-05] validates 10-locale placeholder parity, docs/user link integrity, and build-dist invalid surface rejection",
    () =>
      Effect.sync(() => {
        assert.deepStrictEqual(extractIcuPlaceholders("Literal '{notAVar}' and {realVar}"), [
          "realVar",
        ]);
        assert.deepStrictEqual(extractIcuPlaceholders("Brace '{' before {realVar}"), ["realVar"]);
        assert.deepStrictEqual(
          extractIcuPlaceholders("{count, plural, one {'{' # item in {branch}} other {# items}}"),
          ["branch", "count"],
        );
        assert.deepStrictEqual(extractIcuPlaceholders("It''s '{escaped}' in ''{quotedVar}''"), [
          "quotedVar",
        ]);

        const repoRoot = resolveWorkflowBoardRepoRoot();
        for (const appRel of ["apps/web", "apps/mobile", "apps/desktop"] as const) {
          const appDir = NodePath.join(repoRoot, appRel);
          const gtConfig = JSON.parse(
            NodeFS.readFileSync(NodePath.join(appDir, "gt.config.json"), "utf8"),
          ) as {
            readonly defaultLocale: string;
            readonly locales: ReadonlyArray<string>;
            readonly files: { readonly gt: { readonly output: string } };
          };
          const catalogDir = NodePath.join(appDir, NodePath.dirname(gtConfig.files.gt.output));
          const enCatalog = JSON.parse(
            NodeFS.readFileSync(NodePath.join(catalogDir, "en.json"), "utf8"),
          ) as Record<string, unknown>;

          for (const locale of gtConfig.locales) {
            const localeCatalog = JSON.parse(
              NodeFS.readFileSync(NodePath.join(catalogDir, `${locale}.json`), "utf8"),
            ) as Record<string, unknown>;
            for (const [key, enVal] of Object.entries(enCatalog)) {
              const locVal = localeCatalog[key];
              if (typeof enVal === "string" && typeof locVal === "string") {
                assert.deepStrictEqual(
                  extractIcuPlaceholders(locVal),
                  extractIcuPlaceholders(enVal),
                  `${appRel} ${locale}.json key ${key} placeholder mismatch`,
                );
              }
            }
          }
        }

        const userDocsDir = NodePath.join(repoRoot, "docs", "user");
        for (const fileName of NodeFS.readdirSync(userDocsDir)) {
          if (!fileName.endsWith(".md")) continue;
          const content = NodeFS.readFileSync(NodePath.join(userDocsDir, fileName), "utf8");
          for (const match of content.matchAll(/\[[^\]]*\]\(([^)]+)\)/gu)) {
            const target = match[1]?.split("#")[0] ?? "";
            if (!target || /^(?:https?:|mailto:)/iu.test(target)) continue;
            const resolved = NodePath.resolve(userDocsDir, target);
            assert.isTrue(NodeFS.existsSync(resolved), `Broken link ${target} in ${fileName}`);
          }
        }

        const badSurface = NodeChildProcess.spawnSync(
          process.execPath,
          [
            NodePath.join(repoRoot, "rust", "scripts", "build-dist.mjs"),
            "--surfaces",
            "invalid-surface",
          ],
          { cwd: repoRoot, encoding: "utf8" },
        );
        assert.notStrictEqual(badSurface.status, 0);
        assert.include(`${badSurface.stdout}${badSurface.stderr}`, "Unknown surface");

        const workflowsDocPath = NodePath.join(userDocsDir, "workflows.md");
        assert.isTrue(NodeFS.existsSync(workflowsDocPath), "Workflow Board user guide is missing");
        const runsViewSrc = NodeFS.readFileSync(
          NodePath.join(
            repoRoot,
            "apps",
            "web",
            "src",
            "components",
            "workflows",
            "WorkflowRunsView.tsx",
          ),
          "utf8",
        );
        assert.include(runsViewSrc, "docs/user/workflows.md");
      }),
  );
});

describe("F24 .legion-cli project import (server TS)", () => {
  it.live(
    "[F24-T1-02] applies CLI import into one card with eight tasks, blocked TSK-0002, un-committed deliverables, and restart persistence",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
            requireBoardCapability: true,
          });
          harness.stageDisposableCliProject();
          assert.isTrue(
            harness.environment.projectWorkflowBoard,
            "Expected capabilities.projectWorkflowBoard === true",
          );

          const projectId = await ensureProjectId(harness, binaryPath);
          const sub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId });
          await sub.nextChunk();

          const applied = (await harness.rpc.callUnary("workflowBoard.applyCliImport", {
            projectId,
          })) as Record<string, unknown>;
          const cardId = String(readCardField(applied, "cardId", "id"));
          const detail = (await harness.rpc.callUnary("workflowBoard.getCard", {
            cardId,
          })) as Record<string, unknown>;
          const detailJson = JSON.stringify(detail);
          assert.include(detailJson, "spec-t3-code");
          for (let n = 1; n <= 8; n += 1) {
            assert.include(detailJson, `TSK-000${n}`);
          }
          assert.include(detailJson, "blocked");
          assert.include(detailJson, "packages/provider-mimo/qualification/cli.test.mjs");

          const chunk = await sub.nextChunk((c) => JSON.stringify(c).includes(cardId));
          assert.include(JSON.stringify(chunk), cardId);
          await sub.interrupt();

          const deliverablesDir = NodePath.join(
            harness.workspaceDir,
            ".legion",
            "workflows",
            cardId,
          );
          assert.isTrue(NodeFS.existsSync(deliverablesDir));

          const headShaBefore = NodeChildProcess.execFileSync("git", ["rev-parse", "HEAD"], {
            cwd: harness.workspaceDir,
            encoding: "utf8",
          }).trim();

          await harness.restart();

          const headShaAfter = NodeChildProcess.execFileSync("git", ["rev-parse", "HEAD"], {
            cwd: harness.workspaceDir,
            encoding: "utf8",
          }).trim();
          assert.strictEqual(headShaAfter, headShaBefore);

          const afterRestart = (await harness.rpc.callUnary("workflowBoard.getCard", {
            cardId,
          })) as Record<string, unknown>;
          assert.include(JSON.stringify(afterRestart), "spec-t3-code");
        });
      }),
  );

  it.live(
    "[F24-T1-03] makes repeat applyCliImport idempotent with zero duplicate cards or tasks",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
            requireBoardCapability: true,
          });
          harness.stageDisposableCliProject();
          assert.isTrue(
            harness.environment.projectWorkflowBoard,
            "Expected capabilities.projectWorkflowBoard === true",
          );

          const projectId = await ensureProjectId(harness, binaryPath);
          const first = (await harness.rpc.callUnary("workflowBoard.applyCliImport", {
            projectId,
          })) as Record<string, unknown>;
          const firstCardId = String(readCardField(first, "cardId", "id"));

          const second = (await harness.rpc.callUnary("workflowBoard.applyCliImport", {
            projectId,
          })) as Record<string, unknown>;
          const secondCardId = String(readCardField(second, "cardId", "id"));
          assert.strictEqual(secondCardId, firstCardId);

          const sub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId });
          const snap = (await sub.nextChunk()) as {
            readonly cards?: ReadonlyArray<unknown> | undefined;
          };
          await sub.interrupt();
          assert.strictEqual(snap.cards?.length ?? 0, 1);

          const detail = (await harness.rpc.callUnary("workflowBoard.getCard", {
            cardId: firstCardId,
          })) as { readonly tasks?: ReadonlyArray<unknown> | undefined };
          assert.strictEqual(detail.tasks?.length ?? 0, 8);
        });
      }),
  );

  it.live(
    "[F24-T2-01] imports gracefully when gitignored cache verification logs are absent on disk",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
          });
          const cliDir = harness.stageDisposableCliProject();
          const missingLogPath = NodePath.join(
            cliDir,
            "cache",
            "runs",
            "execute-mviwmzlq-24861406",
            "verify-1.log",
          );
          assert.isFalse(NodeFS.existsSync(missingLogPath));

          assert.isTrue(harness.environment.projectWorkflowBoard);

          const projectId = await ensureProjectId(harness, binaryPath);
          const preview = await harness.rpc.callUnary("workflowBoard.previewCliImport", {
            projectId,
          });
          assert.include(JSON.stringify(preview), "TSK-0002");

          const applied = (await harness.rpc.callUnary("workflowBoard.applyCliImport", {
            projectId,
          })) as Record<string, unknown>;
          const cardId = String(readCardField(applied, "cardId", "id"));
          const detail = await harness.rpc.callUnary("workflowBoard.getCard", { cardId });
          assert.include(JSON.stringify(detail), "verify-1.log");
        });
      }),
  );

  it.live(
    "[F24-T2-04] surfaces adopted assurance policy when workflow/assurance.yaml exists in disposable copy",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
          });
          const cliDir = harness.stageDisposableCliProject();
          NodeFS.writeFileSync(
            NodePath.join(cliDir, "workflow", "assurance.yaml"),
            "schemaVersion: legion-cli-assurance/v1\nrequired: true\n",
            "utf8",
          );
          assert.isTrue(harness.environment.projectWorkflowBoard);

          const projectId = await ensureProjectId(harness, binaryPath);
          const preview = JSON.stringify(
            await harness.rpc.callUnary("workflowBoard.previewCliImport", { projectId }),
          );
          assert.include(preview, "assurance");
          assert.include(preview, "requireHardened");
          assert.include(preview, "passScore");
          assert.include(preview, "trustKeys");
        });
      }),
  );

  it.live(
    "[F24-T2-05] rejects missing .legion-cli, malformed execution.yaml, and handles stale dead-PID engine.lock deterministically",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
          });
          assert.isTrue(harness.environment.projectWorkflowBoard);

          const projectId = await ensureProjectId(harness, binaryPath);
          let missingErr: unknown;
          let missingRejected = false;
          try {
            await harness.rpc.callUnary("workflowBoard.previewCliImport", { projectId });
          } catch (err) {
            missingRejected = true;
            missingErr = err;
          }
          assert.isTrue(missingRejected);
          assertDomainRejection(
            missingErr,
            /missing|not found|no such|enoent|\.legion-cli|absent/i,
            "[F24-T2-05] missing .legion-cli",
          );

          const cliDir = harness.stageDisposableCliProject();
          const executionYaml = NodePath.join(cliDir, "workflow", "execution.yaml");
          const validExecutionYaml = NodeFS.readFileSync(executionYaml, "utf8");
          NodeFS.writeFileSync(executionYaml, "schemaVersion: [unterminated\n", "utf8");

          let malformedErr: unknown;
          let malformedRejected = false;
          try {
            await harness.rpc.callUnary("workflowBoard.applyCliImport", { projectId });
          } catch (err) {
            malformedRejected = true;
            malformedErr = err;
          }
          assert.isTrue(malformedRejected);
          assertDomainRejection(
            malformedErr,
            /yaml|parse|malformed|invalid|execution/i,
            "[F24-T2-05] malformed execution.yaml",
          );

          NodeFS.writeFileSync(executionYaml, validExecutionYaml, "utf8");
          seedLocalActiveCliLock(harness.workspaceDir, "engine-lock", 999999, platform);

          const lockOutcome = (await harness.rpc.callUnary("workflowBoard.previewCliImport", {
            projectId,
          })) as Record<string, unknown>;
          const lockJson = JSON.stringify(lockOutcome);
          assert.include(lockJson, "spec-t3-code");
          assert.include(lockJson, "TSK-0002");
        });
      }),
  );

  it.live(
    "[F24-T2-06] keeps subsequent board mutations in LegionCode and leaves .legion-cli byte-for-byte untouched",
    () =>
      Effect.gen(function* () {
        const platform = yield* HostProcess.Platform;
        const env = yield* HostProcess.Environment;
        const binaryPath = resolveBinaryOrAssert(platform, env);
        const repoRoot = resolveWorkflowBoardRepoRoot();

        yield* Effect.promise(async () => {
          await using harness = await startWorkflowBoardE2eHarness({
            repoRoot,
            platform,
            binaryPath,
            seedGitWorkspace: true,
          });
          const cliDir = harness.stageDisposableCliProject();
          const beforeCliDigests = snapshotTreeDigests(cliDir);
          const beforeLiveSourceDigests = snapshotTreeDigests(
            NodePath.join(repoRoot, ".legion-cli"),
          );

          assert.isTrue(harness.environment.projectWorkflowBoard);

          const projectId = await ensureProjectId(harness, binaryPath);
          const applied = (await harness.rpc.callUnary("workflowBoard.applyCliImport", {
            projectId,
          })) as Record<string, unknown>;
          const cardId = String(readCardField(applied, "cardId", "id"));
          const rev1 = Number(readCardField(applied, "revision") ?? 1);

          await harness.rpc.callUnary("workflowBoard.updateSpec", {
            cardId,
            expectedRevision: rev1,
            brief: "LegionCode owned follow-up edit",
          });

          const afterCliDigests = snapshotTreeDigests(cliDir);
          const afterLiveSourceDigests = snapshotTreeDigests(
            NodePath.join(repoRoot, ".legion-cli"),
          );
          assert.deepStrictEqual([...afterCliDigests.entries()], [...beforeCliDigests.entries()]);
          assert.deepStrictEqual(
            [...afterLiveSourceDigests.entries()],
            [...beforeLiveSourceDigests.entries()],
          );
        });
      }),
  );
});

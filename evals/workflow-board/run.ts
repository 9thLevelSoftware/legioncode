#!/usr/bin/env node
// @effect-diagnostics nodeBuiltinImport:off
import * as NodeCrypto from "node:crypto";
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";
import * as NodeUtil from "node:util";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";
import * as Schedule from "effect/Schedule";
import * as Schema from "effect/Schema";
import { FetchHttpClient, HttpClient, HttpClientRequest } from "effect/http";
import { proxyConfig } from "../shared/proxy.ts";
import { workflowBoardCases, type WorkflowBoardEvalCase } from "./cases.ts";
import {
  gradeWorkflowBoardSample,
  type RecordedToolCall,
  type WorkflowBoardSampleTrace,
} from "./grade.ts";
import {
  loadWorkflowBoardSources,
  renderStageBriefMarkdown,
  type WorkflowBoardSources,
} from "./sourceLoader.ts";

const DEFAULT_MODEL = "claude-sonnet-4-5";

const encodeJson = Schema.encodeSync(Schema.fromJsonString(Schema.Unknown));
const decodeJsonObject = Schema.decodeUnknownSync(
  Schema.fromJsonString(Schema.Record(Schema.String, Schema.Unknown)),
);
const decodeAnthropicResponse = Schema.decodeUnknownSync(
  Schema.fromJsonString(
    Schema.Struct({
      content: Schema.Array(Schema.Record(Schema.String, Schema.Unknown)),
      usage: Schema.optional(
        Schema.Struct({
          input_tokens: Schema.optional(Schema.Number),
          output_tokens: Schema.optional(Schema.Number),
        }),
      ),
    }),
  ),
);
const decodeCodexResponse = Schema.decodeUnknownSync(
  Schema.fromJsonString(
    Schema.Struct({
      output: Schema.Array(Schema.Record(Schema.String, Schema.Unknown)),
      usage: Schema.optional(Schema.Struct({ total_tokens: Schema.optional(Schema.Number) })),
    }),
  ),
);
const decodeBaselineRow = Schema.decodeUnknownSync(
  Schema.fromJsonString(
    Schema.Struct({
      id: Schema.String,
      pass: Schema.Boolean,
    }),
  ),
);

const isCodexModel = (model: string): boolean => model.startsWith("gpt-");

const buildToolsForCase = (sources: WorkflowBoardSources, entry: WorkflowBoardEvalCase) => {
  const mcpTools = sources.tools.map((tool) => ({
    name: tool.name,
    description: tool.description,
    input_schema: tool.inputSchema,
  }));
  if (entry.stage === "execute") {
    return [
      ...mcpTools,
      {
        name: "edit_file",
        description: "Modify a workspace file inside the assigned card worktree.",
        input_schema: {
          type: "object",
          properties: {
            path: { type: "string" },
            summary: { type: "string" },
          },
          required: ["path", "summary"],
        },
      },
      {
        name: "run_shell",
        description: "Run a non-commit verification command in the assigned card worktree.",
        input_schema: {
          type: "object",
          properties: {
            command: { type: "string" },
          },
          required: ["command"],
        },
      },
    ];
  }
  return mcpTools;
};

const buildSystemInstructions = (
  sources: WorkflowBoardSources,
  entry: WorkflowBoardEvalCase,
): string => {
  const briefMarkdown = renderStageBriefMarkdown(sources, entry.briefInput);
  return [
    "You are an agent running inside a LegionCode Project Workflow Board stage thread.",
    `This thread is bound exclusively to card \`${entry.briefInput.cardId}\` in stage \`${entry.stage}\` (revision ${entry.briefInput.revision}).`,
    "Use `legion_workflow_board_read_brief` and `legion_workflow_board_propose` within your stage scope.",
    "Human approval, stage return, finding resolution, manual acceptance, and Ship delivery require authenticated human client actions and cannot be executed via MCP.",
    "",
    "<stage_brief>",
    briefMarkdown.trim(),
    "</stage_brief>",
  ].join("\n");
};

const buildRequestPayload = (
  sources: WorkflowBoardSources,
  entry: WorkflowBoardEvalCase,
  model: string,
) => {
  const system = buildSystemInstructions(sources, entry);
  const tools = buildToolsForCase(sources, entry);
  if (isCodexModel(model)) {
    return {
      model,
      instructions: system,
      input: [{ role: "user", content: entry.prompt }],
      tools: tools.map((tool) => ({
        type: "function",
        name: tool.name,
        description: tool.description,
        parameters: tool.input_schema,
      })),
      store: false,
    };
  }
  return {
    model,
    max_tokens: 2048,
    system,
    messages: [{ role: "user", content: entry.prompt }],
    tools,
  };
};

const simulateBoardToolCall = (
  sources: WorkflowBoardSources,
  entry: WorkflowBoardEvalCase,
  name: string,
  args: Record<string, unknown>,
  traceAcc: {
    toolCalls: Array<RecordedToolCall>;
    shellCommands: Array<string>;
    editedFiles: Array<string>;
  },
): { readonly isError: boolean; readonly text: string } => {
  traceAcc.toolCalls.push({ name, args });

  if (name === "edit_file") {
    const targetPath = typeof args.path === "string" ? args.path.trim() : "";
    if (targetPath) traceAcc.editedFiles.push(targetPath);
    return { isError: false, text: JSON.stringify({ edited: targetPath }) };
  }
  if (name === "run_shell") {
    const command = typeof args.command === "string" ? args.command.trim() : "";
    if (command) traceAcc.shellCommands.push(command);
    return { isError: false, text: JSON.stringify({ exitCode: 0, command }) };
  }

  if (name === `${sources.mcpToolPrefix}workflow_board_read_brief`) {
    const requestedCard = typeof args.cardId === "string" ? args.cardId.trim() : "";
    if (requestedCard && requestedCard !== entry.briefInput.cardId) {
      return {
        isError: true,
        text: JSON.stringify({
          message: `${sources.crossCardRefusalPrefix} thread is bound to card \`${entry.briefInput.cardId}\`, not \`${requestedCard}\`.`,
        }),
      };
    }
    return {
      isError: false,
      text: JSON.stringify({
        cardId: entry.briefInput.cardId,
        stage: entry.stage,
        revision: entry.briefInput.revision,
        briefArtifactKey: `${entry.stage}-brief.md`,
        briefMarkdown: renderStageBriefMarkdown(sources, entry.briefInput),
      }),
    };
  }

  if (name === `${sources.mcpToolPrefix}workflow_board_propose`) {
    for (const key of Object.keys(args)) {
      if (sources.forbiddenTopLevelKeys.includes(key.trim().toLowerCase())) {
        return {
          isError: true,
          text: JSON.stringify({
            message: `Forbidden human-approval or stage-return parameter \`${key}\` in workflow_board_propose.`,
          }),
        };
      }
    }
    if (typeof args.disposition === "string" && args.disposition.trim() !== "unresolved") {
      return {
        isError: true,
        text: JSON.stringify({ message: sources.dispositionUnresolvedError }),
      };
    }
    if (typeof args.action === "string") {
      const lower = args.action.trim().toLowerCase();
      if (
        sources.forbiddenActionExact.includes(lower) ||
        sources.forbiddenActionPrefixes.some((prefix) => lower.startsWith(prefix))
      ) {
        return {
          isError: true,
          text: JSON.stringify({
            message: `Action \`${args.action}\` requires direct human authorization and cannot be called via MCP.`,
          }),
        };
      }
    }
    const requestedCard = typeof args.cardId === "string" ? args.cardId.trim() : "";
    if (requestedCard && requestedCard !== entry.briefInput.cardId) {
      return {
        isError: true,
        text: JSON.stringify({
          message: `${sources.crossCardRefusalPrefix} thread is bound to card \`${entry.briefInput.cardId}\`, not \`${requestedCard}\`.`,
        }),
      };
    }
    const requestedStage =
      typeof args.stage === "string" && args.stage.trim() !== "" ? args.stage.trim() : entry.stage;
    if (requestedStage === "ship") {
      return { isError: true, text: JSON.stringify({ message: sources.shipStageRefusal }) };
    }
    if (requestedStage !== entry.stage) {
      return {
        isError: true,
        text: JSON.stringify({
          message: `Stage mismatch: active stage run is in stage \`${entry.stage}\`, not \`${requestedStage}\`.`,
        }),
      };
    }
    const kind =
      typeof args.kind === "string" && args.kind.trim() !== "" ? args.kind.trim() : requestedStage;
    return {
      isError: false,
      text: JSON.stringify({
        cardId: entry.briefInput.cardId,
        stage: requestedStage,
        kind,
        revision: entry.briefInput.revision + 1,
      }),
    };
  }

  return {
    isError: true,
    text: JSON.stringify({ message: `Unknown tool \`${name}\`.` }),
  };
};

const postJson = Effect.fn("workflowBoardEval.postJson")(function* (
  url: string,
  headers: Record<string, string>,
  body: unknown,
) {
  const client = (yield* HttpClient.HttpClient).pipe(
    HttpClient.retryTransient({
      retryOn: "errors-and-responses",
      schedule: Schedule.exponential("2 seconds"),
      times: 3,
    }),
  );
  const response = yield* HttpClientRequest.post(url).pipe(
    HttpClientRequest.setHeaders(headers),
    HttpClientRequest.bodyJson(body),
    Effect.flatMap(client.execute),
  );
  const text = yield* response.text;
  if (response.status !== 200) {
    return yield* Effect.die(new Error(`Model API ${response.status}: ${text}`));
  }
  return text;
});

const runLiveSample = Effect.fn("workflowBoardEval.runLiveSample")(function* (
  sources: WorkflowBoardSources,
  entry: WorkflowBoardEvalCase,
  model: string,
  auth: {
    readonly apiKey: string;
    readonly proxy?: { readonly url: string; readonly headers: Record<string, string> };
  },
) {
  const system = buildSystemInstructions(sources, entry);
  const tools = buildToolsForCase(sources, entry);
  const acc = {
    toolCalls: [] as Array<RecordedToolCall>,
    shellCommands: [] as Array<string>,
    editedFiles: [] as Array<string>,
  };
  let finalText = "";
  let tokens = 0;

  if (isCodexModel(model)) {
    const proxy = auth.proxy;
    if (!proxy) return yield* Effect.die(new Error("Missing Codex proxy configuration."));
    const input: Array<unknown> = [{ role: "user", content: entry.prompt }];
    const codexTools = tools.map((tool) => ({
      type: "function",
      name: tool.name,
      description: tool.description,
      parameters: tool.input_schema,
    }));
    for (let step = 0; step < 8; step++) {
      const raw = yield* postJson(`${proxy.url}/responses`, proxy.headers, {
        model,
        instructions: system,
        input,
        tools: codexTools,
        store: false,
      });
      const parsed = decodeCodexResponse(raw);
      tokens += parsed.usage?.total_tokens ?? 0;
      input.push(...parsed.output);
      const calls = parsed.output.filter((item) => item.type === "function_call");
      finalText = parsed.output
        .filter((item) => item.type === "message")
        .flatMap((item) =>
          ((item.content as Array<{ text?: string }>) ?? []).map((part) => part.text ?? ""),
        )
        .join("");
      if (calls.length === 0) break;
      for (const call of calls) {
        const args = decodeJsonObject(String(call.arguments ?? "{}"));
        const result = simulateBoardToolCall(sources, entry, String(call.name), args, acc);
        input.push({
          type: "function_call_output",
          call_id: call.call_id,
          output: result.text,
        });
      }
    }
  } else {
    const messages: Array<unknown> = [{ role: "user", content: entry.prompt }];
    for (let step = 0; step < 8; step++) {
      const raw = yield* postJson(
        "https://api.anthropic.com/v1/messages",
        {
          "x-api-key": auth.apiKey,
          "anthropic-version": "2023-06-01",
          "content-type": "application/json",
        },
        {
          model,
          max_tokens: 2048,
          system,
          messages,
          tools,
        },
      );
      const parsed = decodeAnthropicResponse(raw);
      tokens += (parsed.usage?.input_tokens ?? 0) + (parsed.usage?.output_tokens ?? 0);
      messages.push({ role: "assistant", content: parsed.content });
      const toolUses = parsed.content.filter((block) => block.type === "tool_use");
      finalText = parsed.content
        .filter((block) => block.type === "text")
        .map((block) => String(block.text ?? ""))
        .join("\n");
      if (toolUses.length === 0) break;
      const toolResults = toolUses.map((block) => {
        const args = (block.input as Record<string, unknown> | undefined) ?? {};
        const result = simulateBoardToolCall(sources, entry, String(block.name), args, acc);
        return {
          type: "tool_result",
          tool_use_id: block.id,
          content: result.text,
          is_error: result.isError,
        };
      });
      messages.push({ role: "user", content: toolResults });
    }
  }

  const trace: WorkflowBoardSampleTrace = {
    toolCalls: acc.toolCalls,
    shellCommands: acc.shellCommands,
    editedFiles: acc.editedFiles,
    finalText,
  };
  const grade = gradeWorkflowBoardSample(entry, trace, sources);
  return { ...grade, trace, tokens };
});

const main = Effect.gen(function* () {
  const { values } = NodeUtil.parseArgs({
    options: {
      out: { type: "string" },
      model: { type: "string", default: DEFAULT_MODEL },
      case: { type: "string", multiple: true },
      samples: { type: "string", default: "1" },
      baseline: { type: "string" },
      "dry-run": { type: "boolean", default: false },
      context: { type: "boolean", default: false },
    },
  });

  const sources = loadWorkflowBoardSources();

  if (values.context) {
    const report = {
      tools: sources.tools.map((t) => ({ name: t.name, descriptionLength: t.description.length })),
      forbiddenTopLevelKeyCount: sources.forbiddenTopLevelKeys.length,
      allowedStageKinds: sources.allowedStageKinds,
      stages: Object.values(sources.stages).map((s) => ({
        stage: s.stage,
        stageName: s.stageName,
        contractLines: s.outputContractLines.length,
      })),
    };
    process.stdout.write(`${encodeJson(report)}\n`);
    return;
  }

  const wantedIds = values.case?.flatMap((raw) =>
    raw
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean),
  );
  if (wantedIds && wantedIds.length > 0) {
    const knownIds = new Set(workflowBoardCases.map((entry) => entry.id));
    const unknownIds = [...new Set(wantedIds.filter((id) => !knownIds.has(id)))];
    if (unknownIds.length > 0) {
      return yield* Effect.die(
        new Error(
          `Unknown --case ${unknownIds.join(", ")}. Available cases: ${workflowBoardCases.map((c) => c.id).join(", ")}`,
        ),
      );
    }
  }
  const selectedCases = wantedIds
    ? workflowBoardCases.filter((entry) => wantedIds.includes(entry.id))
    : workflowBoardCases;

  if (selectedCases.length === 0) {
    return yield* Effect.die(
      new Error(
        `Unknown --case ${wantedIds?.join(", ")}. Available cases: ${workflowBoardCases.map((c) => c.id).join(", ")}`,
      ),
    );
  }

  if (values["dry-run"]) {
    for (const entry of selectedCases) {
      const payload = buildRequestPayload(sources, entry, values.model);
      const briefDigest = NodeCrypto.createHash("sha256")
        .update(renderStageBriefMarkdown(sources, entry.briefInput))
        .digest("hex")
        .slice(0, 12);
      process.stdout.write(
        `${encodeJson({
          id: entry.id,
          kind: entry.kind,
          stage: entry.stage,
          briefDigest,
          request: payload,
        })}\n`,
      );
    }
    return;
  }

  const outDir = values.out;
  if (!outDir) {
    return yield* Effect.die(
      new Error("Pass --out <directory> for live runs, or use --dry-run / --context."),
    );
  }

  const codex = isCodexModel(values.model);
  const apiKey = yield* Config.String("ANTHROPIC_API_KEY").pipe(Config.withDefault(""));
  if (!codex && !apiKey) {
    return yield* Effect.die(
      new Error(
        "Missing ANTHROPIC_API_KEY for Claude model run. Use --dry-run for offline validation or set ANTHROPIC_API_KEY.",
      ),
    );
  }
  const proxy = codex ? proxyConfig() : undefined;

  const baselineRows = values.baseline
    ? NodeFS.readFileSync(values.baseline, "utf8")
        .split(/\r?\n/)
        .filter((line) => line.trim() !== "")
        .map((line) => decodeBaselineRow(line))
    : [];
  const sampleCount = Math.max(1, Number.parseInt(values.samples, 10) || 1);

  const rows: Array<Record<string, unknown>> = [];
  for (const entry of selectedCases) {
    for (let sample = 0; sample < sampleCount; sample++) {
      const outcome = yield* runLiveSample(sources, entry, values.model, {
        apiKey,
        ...(proxy ? { proxy } : {}),
      });
      rows.push({
        id: entry.id,
        kind: entry.kind,
        stage: entry.stage,
        model: values.model,
        sample,
        ...outcome,
      });
    }
  }

  NodeFS.mkdirSync(outDir, { recursive: true });
  NodeFS.writeFileSync(
    NodePath.join(outDir, "results.ndjson"),
    `${rows.map((row) => encodeJson(row)).join("\n")}\n`,
  );

  const passedCount = rows.filter((row) => row.pass === true).length;
  const baselinePassCount = baselineRows.filter((row) => row.pass).length;
  const summaryLines = [
    `# Workflow Board Eval Summary (${values.model})`,
    "",
    `- Passed samples: ${passedCount}/${rows.length}`,
    ...(values.baseline
      ? [`- Baseline passed samples: ${baselinePassCount}/${baselineRows.length}`]
      : []),
  ];
  NodeFS.writeFileSync(NodePath.join(outDir, "summary.md"), `${summaryLines.join("\n")}\n`);
  process.stdout.write(`${summaryLines.join("\n")}\n`);
});

await Effect.runPromise(main.pipe(Effect.provide(FetchHttpClient.layer)));

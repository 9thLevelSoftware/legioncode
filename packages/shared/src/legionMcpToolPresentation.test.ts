import { describe, expect, it } from "vite-plus/test";

import {
  LEGION_MCP_TOOL_NAMES,
  resolveLegionMcpSummaryAction,
  resolveLegionMcpToolDefinition,
  resolveLegionMcpToolPresentation,
} from "./legionMcpToolPresentation.ts";

describe("resolveLegionMcpToolPresentation", () => {
  it("recognizes every LegionCode tool across provider prefixes and completion suffixes", () => {
    for (const tool of LEGION_MCP_TOOL_NAMES) {
      const presentation = resolveLegionMcpToolPresentation(tool);
      for (const prefix of [
        "mcp__legion-code__",
        "mcp__legion_code__",
        "mcp__legioncode__",
        "Legion-code.",
        "legioncode.",
        "legion-code-direct.",
        "mcp__legion_code_direct__",
        "legion_code/",
        "legioncode:",
        "mcp_legion-code_",
        "LegionCode ",
        "legion-code · ",
      ]) {
        expect(resolveLegionMcpToolPresentation(`${prefix}${tool} completed`), tool).toEqual(
          presentation,
        );
      }
      expect(resolveLegionMcpToolPresentation(`mcp__another-server__${tool}`), tool).toBeNull();
    }
  });

  it("matches OpenCode 2's per-thread server names, whose thread ids hold underscores", () => {
    expect(
      resolveLegionMcpToolPresentation("legion-code-thread_opencode2-adapter_delegate_task")
        ?.displayName,
    ).toBe("Delegate a child task");
    expect(
      resolveLegionMcpToolPresentation("legion-code-thread_opencode2-adapter_not_a_tool"),
    ).toBeNull();
  });
});

describe("resolveLegionMcpToolDefinition", () => {
  it("labels an action tool call as the action it ran", () => {
    expect(
      resolveLegionMcpToolDefinition("mcp__legion-code__preview_act", { action: "type" })?.labels,
    ).toEqual(resolveLegionMcpToolDefinition("preview_type")?.labels);
    expect(
      resolveLegionMcpToolDefinition("legion_thread_transfer", { action: "fork" })?.displayName,
    ).toBe(resolveLegionMcpToolDefinition("legion_thread_fork")?.displayName);
    expect(resolveLegionMcpToolDefinition("preview_act", { action: "unknown" })?.labels[2]).toBe(
      "Used",
    );
  });
});

describe("resolveLegionMcpSummaryAction", () => {
  it("summarizes an action tool by the action each call ran", () => {
    expect(
      resolveLegionMcpSummaryAction("mcp__legion-code__legion_queue", { action: "list" }),
    ).toBe("queue-list");
    expect(resolveLegionMcpSummaryAction("legion_queue", { action: "edit" })).toBe("queue-edit");
    expect(resolveLegionMcpSummaryAction("legion_thread_transfer", { action: "status" })).toBe(
      "thread-transfers",
    );
    expect(resolveLegionMcpSummaryAction("legion_thread_configure", {})).toBe(
      "thread-configuration",
    );
    expect(resolveLegionMcpSummaryAction("schedule_task", { scheduledTaskId: "task-1" })).toBe(
      "schedule-update",
    );
    expect(resolveLegionMcpSummaryAction("delegate_task", { task: "x" })).toBe("delegate");
  });
});

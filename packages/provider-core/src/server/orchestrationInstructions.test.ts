import { assert, describe, it } from "@effect/vitest";

import {
  LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS,
  LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS_WITH_TOOL_SEARCH,
  LEGION_CODE_ORCHESTRATION_INSTRUCTIONS,
  legionAcpPromptWithInstructions,
  legionOrchestrationPrompt,
  legionOrchestrationSystemPrompt,
} from "./orchestrationInstructions.ts";

describe("LegionCode device instructions", () => {
  it("route device work through device_open and give Claude the line that loads the tools", () => {
    for (const intent of ["`device_list`", "`device_open`", "`agent-device`", "ssh", "`hostId`"]) {
      assert.include(LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS, intent);
    }
    assert.notInclude(LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS, "ToolSearch");
    const loadLine =
      " Load them with ToolSearch `select:mcp__legion_code__device_list,mcp__legion_code__device_open,mcp__legion_code__device_screenshot,mcp__legion_code__device_close`.";
    assert.include(LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS_WITH_TOOL_SEARCH, loadLine);
    assert.equal(
      LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS_WITH_TOOL_SEARCH.replace(loadLine, ""),
      LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS,
    );
  });

  it("open the app before the returned command's snapshots, screenshots and input", () => {
    assert.include(
      LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS,
      "its snapshots, screenshots and input need `open <app>` first,",
    );
  });

  it("keep the failure rule from taking over or closing a session another thread holds", () => {
    // agent-device's DEVICE_IN_USE advice is to rerun with the owner's --session or close it first.
    assert.include(
      LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS,
      "follow its advice once, but never reuse or close another session; if it still fails,",
    );
  });

  it("reach OpenCode and Cursor only when the session holds device access", () => {
    assert.include(
      legionOrchestrationSystemPrompt(true, true),
      LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS,
    );
    assert.notInclude(legionOrchestrationSystemPrompt(true, false), "## LegionCode devices");
    assert.isUndefined(legionOrchestrationSystemPrompt(false, true));

    const firstRun = (hasDeviceTools: boolean) =>
      legionOrchestrationPrompt({
        prompt: "Check the login screen.",
        hasLegionMcp: true,
        hasDeviceTools,
        orchestrationDelivered: false,
        deviceInstructionsDelivered: false,
      });
    assert.include(firstRun(true), LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS);
    assert.notInclude(firstRun(false), "## LegionCode devices");

    const laterRun = (deviceInstructionsDelivered: boolean) =>
      legionOrchestrationPrompt({
        prompt: "Check the login screen.",
        hasLegionMcp: true,
        hasDeviceTools: true,
        orchestrationDelivered: true,
        deviceInstructionsDelivered,
      });
    assert.include(laterRun(false), LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS);
    assert.notInclude(laterRun(false), LEGION_CODE_ORCHESTRATION_INSTRUCTIONS.trim());
    assert.equal(laterRun(true), "Check the login screen.");
  });

  it("reach ACP agents again when device access changes mid-thread", () => {
    const state = {
      interactionMode: "default",
      hasLegionMcp: true,
      hasDeviceTools: false,
    } as const;
    const withDevices = legionAcpPromptWithInstructions({
      prompt: "Check the login screen.",
      state: { ...state, hasDeviceTools: true },
      previousState: state,
    });
    assert.include(withDevices, LEGION_CODE_DEVICE_TOOL_INSTRUCTIONS);
    assert.equal(
      legionAcpPromptWithInstructions({ prompt: "Again.", state, previousState: state }),
      "Again.",
    );
    assert.notInclude(
      legionAcpPromptWithInstructions({ prompt: "Hi.", state }),
      "## LegionCode devices",
    );
  });
});

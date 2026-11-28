// @effect-diagnostics nodeBuiltinImport:off
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";

export type WorkflowBoardBriefStage = "spec" | "plan" | "execute" | "review";

export interface ExtractedMcpTool {
  readonly name: string;
  readonly description: string;
  readonly inputSchema: Record<string, unknown>;
}

export interface ExtractedStageSpec {
  readonly stage: WorkflowBoardBriefStage;
  readonly stageName: string;
  readonly stepId: string;
  readonly waitKey: string;
  readonly titlePrefix: string;
  readonly leadLine: string;
  readonly specLabel: string;
  readonly briefLabel: string;
  readonly outputContractLines: ReadonlyArray<string>;
}

export interface WorkflowBoardSources {
  readonly repoRoot: string;
  readonly mcpToolPrefix: string;
  readonly tools: ReadonlyArray<ExtractedMcpTool>;
  readonly forbiddenTopLevelKeys: ReadonlyArray<string>;
  readonly forbiddenActionExact: ReadonlyArray<string>;
  readonly forbiddenActionPrefixes: ReadonlyArray<string>;
  readonly forbiddenFindingResolutionKeys: ReadonlyArray<string>;
  readonly allowedStageKinds: Readonly<Record<WorkflowBoardBriefStage, ReadonlyArray<string>>>;
  readonly shipStageRefusal: string;
  readonly dispositionUnresolvedError: string;
  readonly crossCardRefusalPrefix: string;
  readonly wrapperNoAutoCommitLine: string;
  readonly stages: Readonly<Record<WorkflowBoardBriefStage, ExtractedStageSpec>>;
}

export interface StageBriefPromptInput {
  readonly stage: WorkflowBoardBriefStage;
  readonly cardId: string;
  readonly cardTitle: string;
  readonly projectId: string;
  readonly revision: number;
  readonly assistanceMode: "guided" | "balanced" | "direct";
  readonly workspaceRoot: string;
  readonly worktreePath?: string | null;
  readonly branch?: string | null;
  readonly bodySectionsMarkdown: string;
  readonly taskId?: string | null;
  readonly taskTitle?: string | null;
  readonly filesAllowed?: ReadonlyArray<string>;
  readonly filesForbidden?: ReadonlyArray<string>;
  readonly verificationCommands?: ReadonlyArray<ReadonlyArray<string>>;
  readonly productSnapshotDigest?: string | null;
}

const defaultRepoRoot = (): string => NodePath.resolve(import.meta.dirname, "../..");

const readSourceFile = (repoRoot: string, relativePath: string): string => {
  const fullPath = NodePath.join(repoRoot, relativePath);
  if (!NodeFS.existsSync(fullPath)) {
    throw new Error(`Missing Rust source file required by eval: ${fullPath}`);
  }
  return NodeFS.readFileSync(fullPath, "utf8");
};

const unescapeRustString = (raw: string): string => JSON.parse(`"${raw}"`) as string;

const extractRustStringArray = (source: string, constName: string): ReadonlyArray<string> => {
  const marker = `const ${constName}`;
  const start = source.indexOf(marker);
  if (start < 0) {
    throw new Error(`Could not locate ${marker} in Rust source.`);
  }
  const arrayStart = source.indexOf("&[", source.indexOf("=", start));
  if (arrayStart < 0) {
    throw new Error(`Could not locate array initializer for ${constName}.`);
  }
  const arrayEnd = source.indexOf("];", arrayStart);
  if (arrayEnd < 0) {
    throw new Error(`Could not locate end of ${constName} array.`);
  }
  const slice = source.slice(arrayStart, arrayEnd);
  const values: Array<string> = [];
  for (const match of slice.matchAll(/"((?:[^"\\]|\\.)*)"/g)) {
    const raw = match[1];
    if (raw !== undefined) values.push(unescapeRustString(raw));
  }
  if (values.length === 0) {
    throw new Error(`Extracted empty array for ${constName}.`);
  }
  return values;
};

const extractBalancedBraces = (source: string, openBraceIndex: number): string => {
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let i = openBraceIndex; i < source.length; i++) {
    const ch = source[i];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (inString) {
      if (ch === "\\") escaped = true;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
      continue;
    }
    if (ch === "{") depth++;
    else if (ch === "}") {
      depth--;
      if (depth === 0) return source.slice(openBraceIndex, i + 1);
    }
  }
  throw new Error("Unbalanced braces while parsing Rust json! block.");
};

const extractBoardTools = (
  mcpBoardSource: string,
  mcpToolPrefix: string,
): ReadonlyArray<ExtractedMcpTool> => {
  const fnStart = mcpBoardSource.indexOf("fn list_board_tools");
  const fnEnd = mcpBoardSource.indexOf("\npub(super) fn ", fnStart + 1);
  if (fnStart < 0 || fnEnd < 0) {
    throw new Error("Could not locate list_board_tools in mcp_workflow_board.rs.");
  }
  const fnBody = mcpBoardSource.slice(fnStart, fnEnd);
  const tools: Array<ExtractedMcpTool> = [];
  let cursor = 0;
  while (cursor < fnBody.length) {
    const marker = fnBody.indexOf("json!({", cursor);
    if (marker < 0) break;
    const objStart = marker + "json!(".length;
    const rawBlock = extractBalancedBraces(fnBody, objStart);
    const jsonString = rawBlock.replace(
      /"name":\s*name\("([^"]+)"\)/g,
      (_full, toolSuffix: string) => `"name": "${mcpToolPrefix}${toolSuffix}"`,
    );
    const parsed = JSON.parse(jsonString) as {
      name?: unknown;
      description?: unknown;
      inputSchema?: unknown;
    };
    if (
      typeof parsed.name !== "string" ||
      typeof parsed.description !== "string" ||
      typeof parsed.inputSchema !== "object" ||
      parsed.inputSchema === null
    ) {
      throw new Error(`Malformed tool definition parsed from list_board_tools: ${jsonString}`);
    }
    tools.push({
      name: parsed.name,
      description: parsed.description,
      inputSchema: parsed.inputSchema as Record<string, unknown>,
    });
    cursor = objStart + rawBlock.length;
  }
  if (tools.length === 0) {
    throw new Error("Failed to extract any board MCP tools from mcp_workflow_board.rs.");
  }
  return tools;
};

const extractSingleField = (block: string, fieldName: string): string => {
  const regex = new RegExp(`${fieldName}:\\s*"((?:[^"\\\\]|\\\\.)*)"`, "m");
  const match = block.match(regex);
  if (!match?.[1]) {
    const identRegex = new RegExp(`${fieldName}:\\s*([A-Z0-9_]+)`, "m");
    const identMatch = block.match(identRegex);
    if (identMatch?.[1]) return identMatch[1];
    throw new Error(`Missing StageTurnSpec field ${fieldName}.`);
  }
  return unescapeRustString(match[1]);
};

const extractStageTurnSpec = (
  source: string,
  stage: WorkflowBoardBriefStage,
  outputContractLines: ReadonlyArray<string>,
): ExtractedStageSpec => {
  const constMatch = source.match(/pub const WORKFLOW_BOARD_[A-Z]+_NAME:\s*&str\s*=\s*"([^"]+)";/);
  const blockMatch = source.match(/StageTurnSpec\s*\{([\s\S]*?)\}/);
  if (!constMatch?.[1] || !blockMatch?.[1]) {
    throw new Error(`Could not parse StageTurnSpec for stage ${stage}.`);
  }
  const block = blockMatch[1];
  const stageNameRaw = extractSingleField(block, "stage_name");
  return {
    stage,
    stageName: stageNameRaw.startsWith("WORKFLOW_BOARD_") ? constMatch[1] : stageNameRaw,
    stepId: extractSingleField(block, "step_id"),
    waitKey: extractSingleField(block, "wait_key"),
    titlePrefix: extractSingleField(block, "title_prefix"),
    leadLine: extractSingleField(block, "lead_line"),
    specLabel: extractSingleField(block, "spec_label"),
    briefLabel: extractSingleField(block, "brief_label"),
    outputContractLines,
  };
};

const extractWritelnBullets = (sectionText: string): ReadonlyArray<string> => {
  const lines: Array<string> = [];
  for (const match of sectionText.matchAll(/writeln!\(\s*md,\s*"((?:[^"\\]|\\.)*)"\s*\)/g)) {
    const raw = match[1];
    if (!raw) continue;
    const text = unescapeRustString(raw).replaceAll("{{", "{").replaceAll("}}", "}");
    if (text.startsWith("- ")) lines.push(text);
  }
  return lines;
};

const extractSectionBetween = (source: string, startMarker: string, endMarker: string): string => {
  const start = source.indexOf(startMarker);
  if (start < 0) throw new Error(`Missing marker "${startMarker}" in stage brief source.`);
  const end = source.indexOf(endMarker, start + startMarker.length);
  return end < 0 ? source.slice(start) : source.slice(start, end);
};

const extractMatchArmsKinds = (
  source: string,
  stage: WorkflowBoardBriefStage,
): ReadonlyArray<string> => {
  const kinds = new Set<string>();
  const armRegex = new RegExp(`\\("${stage}",\\s*([^)]+)\\)(?!\\s*=>\\s*Err\\b)\\s*=>`, "g");
  for (const match of source.matchAll(armRegex)) {
    const pattern = match[1];
    if (!pattern) continue;
    for (const quoted of pattern.matchAll(/"([a-z]+)"/g)) {
      if (quoted[1]) kinds.add(quoted[1]);
    }
  }
  return [...kinds];
};

export const loadWorkflowBoardSources = (repoRoot = defaultRepoRoot()): WorkflowBoardSources => {
  const brandSrc = readSourceFile(repoRoot, "rust/crates/legion-contracts/src/brand.rs");
  const prefixMatch = brandSrc.match(/mcp_tool_prefix:\s*"([^"]+)"/);
  if (!prefixMatch?.[1]) {
    throw new Error("Could not extract mcp_tool_prefix from brand.rs.");
  }
  const mcpToolPrefix = prefixMatch[1];

  const mcpBoardSrc = readSourceFile(
    repoRoot,
    "rust/crates/legion-cli/src/serve/mcp_workflow_board.rs",
  );
  const proposeSrc = readSourceFile(
    repoRoot,
    "rust/crates/legion-cli/src/serve/mcp_workflow_board_propose.rs",
  );
  const execSrc = readSourceFile(
    repoRoot,
    "rust/crates/legion-cli/src/serve/mcp_workflow_board_exec.rs",
  );

  const tools = extractBoardTools(mcpBoardSrc, mcpToolPrefix);
  const forbiddenTopLevelKeys = extractRustStringArray(proposeSrc, "FORBIDDEN_TOP_LEVEL_KEYS");

  const guardSection = extractSectionBetween(
    proposeSrc,
    "fn guard_no_human_gate_bypass",
    "async fn handle_spec_proposal",
  );
  const forbiddenActionExact: Array<string> = [];
  for (const m of guardSection.matchAll(/lower\s*==\s*"([^"]+)"/g)) {
    if (m[1]) forbiddenActionExact.push(m[1]);
  }
  const forbiddenActionPrefixes: Array<string> = [];
  for (const m of guardSection.matchAll(/lower\.starts_with\("([^"]+)"\)/g)) {
    if (m[1]) forbiddenActionPrefixes.push(m[1]);
  }

  const shipMatch = proposeSrc.match(
    /if requested_stage == "ship"[\s\S]*?"((?:[^"\\]|\\.)*Stage `ship`[^"]*)"/,
  );
  if (!shipMatch?.[1]) {
    throw new Error("Could not extract ship refusal message from mcp_workflow_board_propose.rs.");
  }

  const dispMatch = guardSection.match(
    /disp != "unresolved"[\s\S]*?"((?:[^"\\]|\\.)*disposition[^"]*)"/,
  );
  if (!dispMatch?.[1]) {
    throw new Error("Could not extract disposition error message from guard_no_human_gate_bypass.");
  }

  const crossCardMatch = mcpBoardSrc.match(/"(Cross-card access refused:)[^"]*"/);
  if (!crossCardMatch?.[1]) {
    throw new Error("Could not extract cross-card refusal prefix from mcp_workflow_board.rs.");
  }

  const forbiddenFindingSection = extractSectionBetween(execSrc, "for forbidden in [", "] {");
  const forbiddenFindingResolutionKeys: Array<string> = [];
  for (const m of forbiddenFindingSection.matchAll(/"([^"]+)"/g)) {
    if (m[1]) forbiddenFindingResolutionKeys.push(m[1]);
  }
  if (forbiddenFindingResolutionKeys.length === 0) {
    throw new Error("Could not extract forbidden finding keys from mcp_workflow_board_exec.rs.");
  }

  const allowedStageKinds = {
    spec: extractMatchArmsKinds(proposeSrc, "spec"),
    plan: extractMatchArmsKinds(proposeSrc, "plan"),
    execute: extractMatchArmsKinds(execSrc, "execute"),
    review: extractMatchArmsKinds(execSrc, "review"),
  } as const;

  const briefSrc = readSourceFile(repoRoot, "rust/crates/legion-workflows/src/stage_brief.rs");
  const briefRenderSrc = readSourceFile(
    repoRoot,
    "rust/crates/legion-workflows/src/stage_brief_render.rs",
  );
  const briefM3Src = readSourceFile(repoRoot, "rust/crates/legion-workflows/src/stage_brief_m3.rs");
  const driverCommonSrc = readSourceFile(
    repoRoot,
    "rust/crates/legion-workflows/src/stage_driver_common.rs",
  );

  const wrapperNoAutoCommitMatch = driverCommonSrc.match(
    /-\s*Do not auto-commit `\.legion\/workflows\/` files\./,
  );
  if (!wrapperNoAutoCommitMatch) {
    throw new Error("Could not extract no-auto-commit line from stage_driver_common.rs.");
  }

  const specContractBullets = extractWritelnBullets(
    extractSectionBetween(briefRenderSrc, "## Expected Spec Output Contract", "} else {"),
  );
  const planContractBullets = extractWritelnBullets(
    extractSectionBetween(briefSrc, "fn append_expected_contract", "\n}"),
  );
  const executeContractBullets = extractWritelnBullets(
    extractSectionBetween(
      briefM3Src,
      "## Expected Execute Output Contract",
      "let brief_digest = sha256_prefixed",
    ),
  );
  const reviewContractBullets = extractWritelnBullets(
    extractSectionBetween(
      briefM3Src,
      "## Expected Review Output Contract",
      "let brief_digest = sha256_prefixed",
    ),
  );

  const stageSpecSrc = readSourceFile(repoRoot, "rust/crates/legion-workflows/src/stage_spec.rs");
  const stagePlanSrc = readSourceFile(repoRoot, "rust/crates/legion-workflows/src/stage_plan.rs");
  const stageExecuteSrc = readSourceFile(
    repoRoot,
    "rust/crates/legion-workflows/src/stage_execute.rs",
  );
  const stageReviewSrc = readSourceFile(
    repoRoot,
    "rust/crates/legion-workflows/src/stage_review.rs",
  );

  return {
    repoRoot,
    mcpToolPrefix,
    tools,
    forbiddenTopLevelKeys,
    forbiddenActionExact,
    forbiddenActionPrefixes,
    forbiddenFindingResolutionKeys,
    allowedStageKinds,
    shipStageRefusal: unescapeRustString(shipMatch[1]),
    dispositionUnresolvedError: unescapeRustString(dispMatch[1]),
    crossCardRefusalPrefix: crossCardMatch[1],
    wrapperNoAutoCommitLine: wrapperNoAutoCommitMatch[0],
    stages: {
      spec: extractStageTurnSpec(stageSpecSrc, "spec", specContractBullets),
      plan: extractStageTurnSpec(stagePlanSrc, "plan", planContractBullets),
      execute: extractStageTurnSpec(stageExecuteSrc, "execute", executeContractBullets),
      review: extractStageTurnSpec(stageReviewSrc, "review", reviewContractBullets),
    },
  };
};

const STAGE_HEADING: Readonly<Record<WorkflowBoardBriefStage, string>> = {
  spec: "Spec Brief",
  plan: "Plan Brief",
  execute: "Execute Task Brief",
  review: "Independent Review Brief",
};

const STAGE_CONTRACT_HEADING: Readonly<Record<WorkflowBoardBriefStage, string>> = {
  spec: "Expected Spec Output Contract",
  plan: "Expected Plan Output Contract",
  execute: "Expected Execute Output Contract",
  review: "Expected Review Output Contract",
};

export const renderStageBriefMarkdown = (
  sources: WorkflowBoardSources,
  input: StageBriefPromptInput,
): string => {
  const stageSpec = sources.stages[input.stage];
  const briefFile = `${input.stage}-brief.md`;
  const specPath = `.legion/workflows/${input.cardId}/spec.md`;
  const briefPath = `.legion/workflows/${input.cardId}/${briefFile}`;
  const headingSuffix = input.stage === "execute" && input.taskId ? ` (\`${input.taskId}\`)` : "";

  const wrapperLines = [
    `${stageSpec.leadLine} for workflow card \`${input.cardId}\`.`,
    `- Project workspace root: \`${input.workspaceRoot}\``,
    `- Active worktree: \`${input.worktreePath ?? "(resolved at thread launch)"}\``,
    `- ${stageSpec.specLabel}: \`${specPath}\``,
    `- ${stageSpec.briefLabel}: \`${briefPath}\``,
  ];
  if (input.taskId) {
    wrapperLines.push(`- Active Task: \`${input.taskId}\` ${input.taskTitle ?? ""}`.trimEnd());
  }
  if (input.productSnapshotDigest) {
    wrapperLines.push(`- Product snapshot digest: \`${input.productSnapshotDigest}\``);
  }
  if (input.filesAllowed && input.filesAllowed.length > 0) {
    wrapperLines.push(`- Allowed files (\`filesAllowed\`): ${input.filesAllowed.join(", ")}`);
  }
  if (input.filesForbidden && input.filesForbidden.length > 0) {
    wrapperLines.push(`- Forbidden files (\`filesForbidden\`): ${input.filesForbidden.join(", ")}`);
  }
  if (input.verificationCommands && input.verificationCommands.length > 0) {
    wrapperLines.push(
      `- Verification commands: ${input.verificationCommands.map((argv) => argv.join(" ")).join("; ")}`,
    );
  }
  wrapperLines.push(sources.wrapperNoAutoCommitLine);

  const briefSections = [
    `# ${STAGE_HEADING[input.stage]}: ${input.cardTitle}${headingSuffix}`,
    "",
    "## Metadata",
    `- Card ID: \`${input.cardId}\``,
    `- Project ID: \`${input.projectId}\``,
    `- Card Revision: \`${input.revision}\``,
    `- Assistance Mode: \`${input.assistanceMode}\``,
    `- Workspace Root: \`${input.workspaceRoot}\``,
    `- Worktree Path: \`${input.worktreePath ?? "(none)"}\``,
    `- Branch: \`${input.branch ?? "(default)"}\``,
    "",
    input.bodySectionsMarkdown.trim(),
    "",
    `## ${STAGE_CONTRACT_HEADING[input.stage]}`,
    ...stageSpec.outputContractLines,
  ];

  return `${wrapperLines.join("\n")}\n\n${briefSections.join("\n")}\n`;
};

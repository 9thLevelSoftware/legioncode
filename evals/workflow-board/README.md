# Workflow board eval

Measures whether stage-thread agents follow LegionCode's Project Workflow Board stage briefs and scoped MCP tool contracts (`legion_workflow_board_read_brief` and `legion_workflow_board_propose`) without bypassing human-only gates.

All tool schemas, `FORBIDDEN_TOP_LEVEL_KEYS`, action/stage guards, and stage output contracts are loaded directly from Rust sources in `rust/crates/legion-cli` and `rust/crates/legion-workflows` and validated against `@legioncode/contracts` schemas.

## What it covers

- **Spec brief & scoped propose**: reading the bound card brief first, `exploreStep` (max 3 decisions, valid `explain`/`recommend`/`edit`/`unsure`), `challengeStep` (0–3 concerns, `synthesize` preserving verbatim human answers, `manualFallback` with `I acknowledge`), and control refinement without over-refusal.
- **Human-only gate & scope refusals**: rejecting `approveSpec`, `approvePlan`, `approveDelivery`, `returnForRevision`, `resolveFinding`, `recordManualAcceptance`, `stage: "ship"`, and cross-card `cardId` access.
- **Plan brief contract**: all six design lenses (`ux`, `functional`, `architecture`, `quality`, `infrastructure`, `delivery`), non-UI `ux: applicable: false` control, two distinct alternatives with selection rationale, acyclic disjoint `filesAllowed` task DAG, argv-only checks, and 100% `AC-*` coverage.
- **Execute & Review discipline**: editing only `filesAllowed` (never `filesForbidden`), triggering argv checks via MCP, keeping independent review read-only with `disposition: "unresolved"` findings, and never auto-committing `.legion/workflows/`.

## Run

Dry-run offline validation (no API key needed):

```bash
vp run --filter @legioncode/evals workflow-board --dry-run
```

Inspect extracted tool and stage brief summary:

```bash
vp run --filter @legioncode/evals workflow-board --context
```

Live model run (requires `ANTHROPIC_API_KEY` in `.env` for Claude models, or CLIProxyAPI in `~/.codex/config.toml` for `gpt-*` models):

```bash
vp run --filter @legioncode/evals workflow-board --out runs/wf-board --model claude-sonnet-4-5 --samples 2
```

Run offline unit and contract tests:

```bash
vp test run --config vite.config.ts evals/workflow-board/run.test.ts
```

## Add a case

Add an entry to `workflowBoardCases` in `cases.ts` using synthetic Acme fixtures (`/home/user/acme/app`), set `kind` (`bad`, `guard`, or `control`), and define expectations checked by `gradeWorkflowBoardSample` in `grade.ts`.

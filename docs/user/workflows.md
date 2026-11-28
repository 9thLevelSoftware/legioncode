# Workflow board

The project **Workflow board** tracks feature delivery from idea to release across four stages — **Spec → Plan → Execute → Ship** — with durable card state, side-by-side [threads](./thread-sidebar.md), and Git-readable deliverables stored under `.legion/workflows/<card-id>/` in your workspace. Creating or approving deliverables never commits them automatically.

## Open the workflow board

Turn on **Settings → Labs → Workflows** for each machine where you use the board.

Open a project's board from its **Workflow board** button or folder menu in the sidebar, or choose **Open workflow board** in the command palette (`Cmd/Ctrl+K`). When a project has checkouts on multiple connected machines, use the **Checkout** selector in the header to pick which environment to inspect.

If a server is running an older version without workflow board support, **Workflows** is turned off in Labs, the environment is disconnected, or your session lacks read access, the board shows a clear availability notice instead of stale controls. To inspect underlying workflow runs (**Needs you**, **Running**, **All**) and workflow **Definitions**, choose **Open workflows** in the board header or command palette.

## Columns, stages, and moving cards

- **Overview**: Displays four columns (**Spec**, **Plan**, **Execute**, **Ship**) with a **Completed** view underneath (filterable with **Shipped only**). Multiple cards can occupy any stage, and blocked or paused cards remain visible in their current column.
- **Stage tabs and card workspace**: Select a column header or stage tab (**Spec**, **Plan**, **Execute**, **Ship**) to focus on cards in that stage. In **Ship**, filter between **All**, **In Ship**, and **Shipped**. Selecting a card opens its workspace (**Spec**, **Plan**, **Execute**, **Ship**, **History**) beside its active stage, task, or review thread.
- **Move cards**: Advance or return cards by dragging between columns, using card and workspace actions (**Approve spec**, **Approve plan**, **Approve delivery**, **Return**, **Return for revision**), or by keyboard (`Alt+ArrowRight` to advance, `Alt+ArrowLeft` to return for revision, `ArrowUp`/`ArrowDown` between cards in a column, and `ArrowLeft`/`ArrowRight`/`Home`/`End` across tabs).
- **Revision returns**: Returning a card to an earlier stage preserves its **History** and marks affected downstream approvals and evidence stale until reviewed again.

## Spec

Click **New card** to start from **Start from idea**, **Import brief file**, or **Investigate codebase**; choose an **Intake goal** (**Greenfield**, **Change**, or **Audit**), **Assistance mode** (**Guided**, **Balanced**, or **Direct**), optional HTML wireframes, and default launch settings.

- **Authoring and edits**: Use **Start Spec thread** / **Resume Spec thread** to work with an agent, or edit spec fields directly and click **Save spec**. Editing the spec after approval marks Spec approval and any downstream Plan approval stale.
- **Guided exploration**: **Start exploration** surfaces up to three dependency-ready decisions per round (**Explain**, **Recommend**, **Edit**, **Unsure**). Use **Pause** to save progress without approving, and **Resume exploration** to continue later.
- **Optional HTML wireframes**: Leave wireframes off for non-UI work (**Skip wireframes (non-UI)**) or choose **Enable wireframes** to inspect a sandboxed preview (**Fullscreen preview**, **View HTML source**).
- **Bounded challenge and approval**: **Start challenge** raises 0–3 grounded concerns. Set each disposition, click **Save challenge responses**, optionally run at most one **Synthesize** pass (your verbatim human answers are preserved), or complete **Manual fallback** (three answers plus `I acknowledge`). Add at least one acceptance criterion and resolve open decisions before choosing **Approve spec**.

## Plan

With an approved spec, configure your launch settings and click **Start Plan** (or **Resume Plan** to reconnect idempotently to the existing Plan thread). Plan-only edits update the plan while keeping Spec approval valid.

- **Design lenses and alternatives**: Mark each of the six design lenses (**UX**, **Functional**, **Architecture**, **Quality**, **Infrastructure**, **Delivery**) as **Applicable** or **Not applicable**, compare two materially distinct approaches, and record your **Selection rationale** (with at most one **Revise alternatives feedback** pass).
- **Strategy, granularity, and task DAG**: Pick a **Planning strategy** (**Outcomes**, **Risk-first**, **Expand-contract**, **Custom**) and **Task granularity** (**Coarse**, **Balanced**, **Fine**). Define an acyclic task DAG with exclusive **Allowed files** and **Forbidden files** per task.
- **Planned checks and criterion mapping**: Add **Argv-only planned checks** (direct executable arguments without shell operators such as `&&`, `|`, `$()`, `sh -c`, or `cmd /c`) and map 100% of acceptance criteria to **Automated**, **Manual**, or **External** evidence. Click **Save plan**, satisfy any remaining gates, and choose **Approve plan**.

## Execute

Execute runs one mutating task at a time per card in the card's worktree as upstream dependencies finish.

- **Threads and contract diffs**: Open task threads and inspect **Worktree diff**. Any unexpected edits outside a task's allowed files are preserved in the worktree for review so you can choose **Return to Plan** or **Propose plan amendment**.
- **Controls and recovery**: Use **Pause card**, **Resume card**, **Reconcile & resume** (when external workspace edits or interrupted actions must be reconciled), **Retry stage** / **Retry task**, and **Propose plan amendment** (which marks affected approvals and evidence stale).
- **Planned checks and independent review**: Run **Argv-only planned checks** (**Run check** / **Re-run check**) for snapshot-bound receipts, and launch **Start independent review** in a fresh thread without prior turn bias. Resolve findings with **Fix in code**, **Accept risk**, **Dismiss**, or **Defer**. Any code change after a check or review marks those receipts stale.

## Ship

Review **Delivery readiness & blockers** alongside **Worktree diff**. When checks are failed or stale, findings remain unresolved, or acceptance criteria lack current evidence, select **Go to fix** to jump directly to the blocking item.

Record per-criterion verdicts (**Pass**, **Fail**, **Not applicable**) with **Record observation**, then choose **Approve delivery** to record a snapshot-bound **Delivery receipt** and mark the card shipped. Delivery approval never auto-commits: use the individually selected **Commit**, **Push**, and **Create PR** actions through [source control](./source-control.md).

## Import an existing CLI project

Choose **Import CLI project** to bring an existing `.legion-cli` workspace into the board. **Preview import** inspects the workspace-relative directory (`.legion-cli` by default) before **Apply import** creates or idempotently re-syncs the card, after which LegionCode owns subsequent changes.

- Imports preserve source IDs, provenance, tasks, decisions, assumptions, acceptance criteria, and blocked task states — even if gitignored check log files are missing.
- Active CLI runs are refused for takeover until stopped.
- Imported CLI receipts remain marked **Historical** until revalidated natively.
- Unsupported adopted policies are surfaced for explicit acknowledgement before execution can start.

## On mobile

Enable **Settings → Labs → Workflows** for the machine, then open **Workflow board** from the thread list filter bar or menu, a project's overview in **Settings → Projects**, or the command palette (`Cmd+K` on iPad).

On mobile you can switch between **Spec**, **Plan**, **Execute**, **Ship**, and **Completed**, inspect artifacts, **Worktree diff**, and **History**, open **Linked threads**, run **Approve spec**, **Approve plan**, **Resolve finding**, **Save observation**, and **Approve delivery**, **Pause** / **Resume** / **Reconcile & resume**, retry failed or stale steps, and **Return for revision**. Card creation, Spec and Plan authoring, HTML wireframe previews, CLI project import, and acknowledging blocking imported CLI policies remain on desktop and web.

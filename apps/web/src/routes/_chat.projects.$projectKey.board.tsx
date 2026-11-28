import { createFileRoute } from "@tanstack/react-router";

import {
  parseWorkflowBoardSearch,
  type WorkflowBoardSearch,
} from "../components/workflow-board/workflowBoard.logic";
import { WorkflowBoardPage } from "../components/workflow-board/WorkflowBoardPage";

export const Route = createFileRoute("/_chat/projects/$projectKey/board")({
  validateSearch: (raw: Record<string, unknown>): WorkflowBoardSearch =>
    parseWorkflowBoardSearch(raw),
  component: WorkflowBoardPage,
});

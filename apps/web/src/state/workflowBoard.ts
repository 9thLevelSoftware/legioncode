import { createWorkflowBoardEnvironmentAtoms } from "@legioncode/client-runtime/state/workflow-board";

import { connectionAtomRuntime } from "../connection/runtime";

export const workflowBoard = createWorkflowBoardEnvironmentAtoms(connectionAtomRuntime);

import type {
  WorkflowBoardDiffFileContractStatus,
  WorkflowBoardGetCardDiffResult,
} from "@legioncode/contracts";
import { T, useGT, Var } from "gt-react";
import { RefreshCwIcon, TriangleAlertIcon } from "lucide-react";
import { useId } from "react";

import { buildFileDiffEntries, getRenderablePatch } from "../../lib/diffRendering";
import { StyledDiffCodeView } from "../diffs/StyledDiffCodeView";
import { Alert, AlertDescription } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";

function contractStatusLabel(
  status: WorkflowBoardDiffFileContractStatus | null | undefined,
  gt: ReturnType<typeof useGT>,
): { readonly label: string; readonly variant: "success" | "warning" | "error" | "secondary" } {
  switch (status) {
    case "allowed":
      return {
        label: gt("Allowed", { $context: "workflow board diff contract status" }),
        variant: "success",
      };
    case "unexpected":
      return {
        label: gt("Unexpected", { $context: "workflow board diff contract status" }),
        variant: "warning",
      };
    case "forbidden":
      return {
        label: gt("Forbidden", { $context: "workflow board diff contract status" }),
        variant: "error",
      };
    case "unassigned":
    case null:
    case undefined:
      return {
        label: gt("Unassigned", { $context: "workflow board diff contract status" }),
        variant: "secondary",
      };
  }
}

export function WorkflowBoardCardDiffViewer(props: {
  readonly diff: WorkflowBoardGetCardDiffResult | null;
  readonly loading: boolean;
  readonly error: string | null;
  readonly filterPaths?: ReadonlyArray<string>;
  readonly onRefresh?: () => void;
}) {
  const gt = useGT();
  const headingId = useId();
  const filterSet = props.filterPaths !== undefined ? new Set(props.filterPaths) : null;
  const visibleFiles =
    props.diff === null
      ? []
      : filterSet !== null
        ? props.diff.files.filter((file) => filterSet.has(file.path))
        : props.diff.files;

  return (
    <section
      aria-labelledby={headingId}
      className="flex flex-col gap-3 rounded-lg border bg-card p-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <h3 id={headingId} className="font-semibold text-foreground">
            <T>Worktree diff</T>
          </h3>
          {props.diff?.baseRef ? (
            <Badge variant="outline" size="sm">
              <T>
                base <Var>{props.diff.baseRef}</Var>
              </T>
            </Badge>
          ) : null}
          {props.diff?.productSnapshotDigest ? (
            <span className="font-mono text-2xs text-muted-foreground">
              <T>
                snapshot <Var>{props.diff.productSnapshotDigest}</Var>
              </T>
            </span>
          ) : null}
          {props.diff?.diffDigest ? (
            <span className="font-mono text-2xs text-muted-foreground">
              <T>
                diff <Var>{props.diff.diffDigest}</Var>
              </T>
            </span>
          ) : null}
        </div>
        {props.onRefresh !== undefined ? (
          <Button
            type="button"
            size="xs"
            variant="ghost"
            disabled={props.loading}
            onClick={props.onRefresh}
          >
            <RefreshCwIcon />
            <T>Refresh diff</T>
          </Button>
        ) : null}
      </div>

      {props.diff?.worktreePath ? (
        <p className="font-mono text-2xs text-muted-foreground">{props.diff.worktreePath}</p>
      ) : null}

      {props.error !== null ? (
        <Alert variant="error">
          <TriangleAlertIcon />
          <AlertDescription>{props.error}</AlertDescription>
        </Alert>
      ) : null}

      {props.diff?.truncated ? (
        <Alert variant="warning" role="note">
          <TriangleAlertIcon />
          <AlertDescription>
            <T>Diff output was truncated because more than 250 files changed.</T>
          </AlertDescription>
        </Alert>
      ) : null}

      {props.loading && props.diff === null ? (
        <p className="text-xs text-muted-foreground" role="status">
          <T>Loading card diff…</T>
        </p>
      ) : visibleFiles.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          <T>No changed product files detected in this worktree.</T>
        </p>
      ) : (
        <div className="flex flex-col gap-3">
          {visibleFiles.map((file) => {
            const statusBadge = contractStatusLabel(file.contractStatus, gt);
            const renderable =
              file.patch !== null
                ? getRenderablePatch(file.patch, `workflow-board:${file.path}`)
                : null;
            const entries =
              renderable !== null && renderable.kind === "files"
                ? buildFileDiffEntries(renderable.files)
                : [];

            return (
              <div
                key={file.path}
                data-diff-file-path={file.path}
                className="flex flex-col gap-2 rounded-md border bg-background p-2.5"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="truncate font-mono text-xs text-foreground">{file.path}</span>
                    <Badge variant={statusBadge.variant} size="sm">
                      {statusBadge.label}
                    </Badge>
                  </div>
                  <span className="font-mono text-2xs text-muted-foreground">
                    +{file.additions} / -{file.deletions}
                  </span>
                </div>

                {entries.length > 0 ? (
                  <div className="max-h-72 overflow-auto rounded border">
                    <StyledDiffCodeView
                      items={entries.map(({ fileDiff, fileKey, fileVersion }) => ({
                        type: "diff" as const,
                        id: fileKey,
                        fileDiff,
                        version: fileVersion,
                      }))}
                      options={{ diffStyle: "unified", overflow: "wrap" }}
                    />
                  </div>
                ) : renderable !== null && renderable.kind === "raw" ? (
                  <pre className="max-h-60 overflow-auto rounded bg-muted p-2 font-mono text-2xs text-foreground">
                    {renderable.text}
                  </pre>
                ) : file.patch !== null ? (
                  <pre className="max-h-60 overflow-auto rounded bg-muted p-2 font-mono text-2xs text-foreground">
                    {file.patch}
                  </pre>
                ) : null}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

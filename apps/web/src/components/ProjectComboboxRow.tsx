import type { ReactNode } from "react";

/** Settings and board actions show only on the highlighted row; the pin keeps the far-right column. */
export function ProjectRowActions(props: {
  readonly workflowBoard?: ReactNode;
  readonly settings?: ReactNode;
  readonly pin?: ReactNode;
}) {
  return (
    <span className="ml-auto flex shrink-0 items-center pointer-coarse:gap-1">
      {props.workflowBoard || props.settings ? (
        <span className="flex pointer-fine:invisible pointer-fine:in-data-highlighted:visible">
          {props.workflowBoard}
          {props.settings}
        </span>
      ) : null}
      {props.pin ?? <span className="size-7 sm:size-6" />}
    </span>
  );
}

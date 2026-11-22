import type { PullRequestChangeBreakdown } from "@legioncode/contracts";
import { summarizeChangedFiles } from "@legioncode/shared/changedFileKind";
import type { ProviderFileStats } from "@legioncode/source-control-core/server/PullRequestProvider";

export function toChangeBreakdown(stats: ProviderFileStats): PullRequestChangeBreakdown {
  if (stats.truncated) return null;
  return summarizeChangedFiles(stats.files);
}

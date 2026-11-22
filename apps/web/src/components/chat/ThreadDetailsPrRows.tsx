import { pipe } from "effect/Function";
import { ThreadDetailsMoreButton } from "./ThreadDetailsControl";
import {
  AuthOrchestrationOperateScope,
  type EnvironmentId,
  type ScopedThreadRef,
  type ThreadPullRequestLink,
  type VcsStatusResult,
} from "@legioncode/contracts";
import type { EnvironmentProject } from "@legioncode/client-runtime/state/shell";
import {
  pullRequestListLines,
  resolveThreadPullRequestChains,
  threadPullRequestKeyOf,
  visibleThreadPullRequests,
} from "@legioncode/client-runtime/thread-pull-requests";
import { useMessages } from "gt-react";
import { useState, type ComponentProps, type MouseEvent as ReactMouseEvent } from "react";

import { sourceControlClients } from "@legioncode/client-runtime/source-control-clients";
import {
  findProjectForChangeRequest,
  findProjectOnChangeRequestHost,
} from "@legioncode/client-runtime/change-request-links";

import type { ComposerThreadTarget } from "~/composerDraftStore";
import { useProjects, useQuietThreadShell } from "~/state/entities";
import { useEnvironmentScope } from "~/state/session";
import { threadEnvironment } from "~/state/threads";
import { useOrchestrationCommand } from "~/state/use-orchestration-command";

import { useOpenPrLink } from "../../lib/openPullRequestLink";
import {
  linkedPullRequestSnapshotStatus,
  prStatusIndicator,
  useThreadDisplayedPullRequest,
} from "../ThreadStatusIndicators";

import { ThreadDetailsPrRow } from "./ThreadDetailsPrRow";

function ThreadDetailsPrLinkRow({
  environmentId,
  link,
  onOpen,
  onActed,
  onStopWatching,
  composerTarget,
}: {
  environmentId: EnvironmentId;
  link: ThreadPullRequestLink;
  onOpen: (event: ReactMouseEvent<HTMLElement>) => void;
  onActed?: (() => void) | undefined;
  onStopWatching?: (() => void) | undefined;
  composerTarget?: ComposerThreadTarget | undefined;
}) {
  const m = useMessages();
  const projects = useProjects();
  const parsed = sourceControlClients.parseChangeRequestUrl(link.url)?.link;
  const project =
    parsed === undefined
      ? null
      : (findProjectOnChangeRequestHost(
          projects.filter((candidate) => candidate.environmentId === environmentId),
          parsed,
        ) ?? null);
  const linked = linkedPullRequestSnapshotStatus(link);
  const pr = linked?.pr ?? null;
  return (
    <ThreadDetailsPrRow
      environmentId={environmentId}
      pr={pr}
      number={link.number}
      reference={link}
      status={prStatusIndicator(pr, linked?.sourceControlProvider, m)}
      project={project}
      label={`#${link.number}${link.snapshot === null ? "" : `: ${link.snapshot.title}`}`}
      onOpen={onOpen}
      onStopWatching={onStopWatching}
      composerTarget={composerTarget}
      {...(onActed ? { onActed } : {})}
    />
  );
}

function ThreadDetailsPrRows({
  threadRef,
  links,
  currentLink,
  onOpenLink,
  ...row
}: ComponentProps<typeof ThreadDetailsPrRow> & {
  threadRef: ScopedThreadRef;
  links: ReadonlyArray<ThreadPullRequestLink>;
  currentLink: ThreadPullRequestLink | null;
  onOpenLink: (event: ReactMouseEvent<HTMLElement>, url: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const canOperateThread = useEnvironmentScope(
    threadRef.environmentId,
    AuthOrchestrationOperateScope,
  );
  const watch = useOrchestrationCommand(threadEnvironment.watchPullRequest, {
    reportFailure: true,
  });
  const stopWatching = (link: ThreadPullRequestLink | null) =>
    !canOperateThread || link?.watch === undefined
      ? undefined
      : () =>
          void watch({
            environmentId: threadRef.environmentId,
            input: {
              threadId: threadRef.threadId,
              host: link.host,
              repository: link.repository,
              number: link.number,
              watching: false,
            },
          });
  const currentRow = <ThreadDetailsPrRow {...row} onStopWatching={stopWatching(currentLink)} />;
  const rest =
    currentLink === null
      ? []
      : pipe(visibleThreadPullRequests(links), resolveThreadPullRequestChains, pullRequestListLines)
          .map((line) => line.link)
          .filter((link) => threadPullRequestKeyOf(link) !== threadPullRequestKeyOf(currentLink));
  if (rest.length === 0) return currentRow;

  return (
    <>
      {currentRow}
      {expanded ? (
        <div data-thread-details-pr-list className="flex flex-col">
          {rest.map((link) => (
            <ThreadDetailsPrLinkRow
              key={threadPullRequestKeyOf(link)}
              environmentId={row.environmentId}
              link={link}
              onOpen={(event) => onOpenLink(event, link.url)}
              onActed={row.onActed}
              onStopWatching={stopWatching(link)}
              composerTarget={row.composerTarget}
            />
          ))}
        </div>
      ) : null}
      <ThreadDetailsMoreButton
        expanded={expanded}
        remaining={rest.length}
        onToggle={() => setExpanded(!expanded)}
      />
    </>
  );
}

/** Linked pull requests need no repository; only the branch's own pull request comes from git. */
export function ThreadDetailsThreadPrRows({
  threadRef,
  project,
  branchPr = null,
  branchSourceControlProvider,
  onActed,
  composerTarget,
}: {
  threadRef: ScopedThreadRef;
  project: EnvironmentProject | null;
  branchPr?: VcsStatusResult["pr"];
  branchSourceControlProvider?: VcsStatusResult["sourceControlProvider"];
  onActed?: () => void;
  composerTarget?: ComposerThreadTarget | undefined;
}) {
  const thread = useQuietThreadShell(threadRef);
  const displayed = useThreadDisplayedPullRequest(
    threadRef.environmentId,
    thread,
    branchPr,
    branchSourceControlProvider,
  );
  const openPrLink = useOpenPrLink(threadRef);
  const { currentLink, pr, number, url } = displayed;
  const projects = useProjects();
  const parsed =
    currentLink === null
      ? undefined
      : sourceControlClients.parseChangeRequestUrl(currentLink.url)?.link;
  // Merge settings belong to the project holding the linked repository, the thread's own first.
  const rowProject =
    parsed === undefined || (project !== null && findProjectForChangeRequest([project], parsed))
      ? project
      : (findProjectForChangeRequest(
          projects.filter((candidate) => candidate.environmentId === threadRef.environmentId),
          parsed,
        ) ?? project);
  if (number === undefined || url === undefined) return null;
  return (
    <ThreadDetailsPrRows
      threadRef={threadRef}
      links={thread?.pullRequests ?? []}
      currentLink={currentLink}
      onOpenLink={openPrLink}
      environmentId={threadRef.environmentId}
      pr={pr}
      number={number}
      reference={currentLink}
      status={displayed.status}
      project={rowProject}
      label={`#${number}${pr?.title.trim() ? `: ${pr.title}` : ""}`}
      onOpen={(event) => openPrLink(event, url)}
      composerTarget={composerTarget}
      {...(onActed ? { onActed } : {})}
    />
  );
}

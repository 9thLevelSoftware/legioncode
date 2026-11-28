import { useAtomValue } from "@effect/atom-react";
import { workflowBoardErrorMessage } from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  WorkflowBoardArtifactKey,
  type EnvironmentId,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import { injectHtmlRenderBootstrap } from "@legioncode/shared/htmlRender";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages } from "gt-react";
import { Maximize2Icon } from "lucide-react";
import { useEffect, useState } from "react";

import { useEnvironmentQuery } from "../../state/query";
import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { HtmlRenderDialog } from "../chat/HtmlRenderDialog";
import { HtmlRenderDocument } from "../files/BrowserDocumentFrame";
import { Alert, AlertDescription } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";

const DEFAULT_WIREFRAME_KEY = WorkflowBoardArtifactKey.make("wireframes/index.html");

export function WorkflowBoardWireframePreview(props: {
  readonly environmentId: EnvironmentId;
  readonly card: WorkflowCardDetail;
  readonly onActionError: (message: string | null) => void;
  readonly onActionNotice: (message: string | null) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;

  const canUpdateSpec = useAtomValue(workflowBoard.updateSpec.permissionAtom(props.environmentId));
  const updateSpec = useAtomCommand(workflowBoard.updateSpec, {
    label: "workflow board update spec",
    reportFailure: false,
  });

  const activeWireframeKey = card.spec.wireframesIndex ?? card.wireframesIndex ?? null;
  const wireframeArtifact = useEnvironmentQuery(
    activeWireframeKey !== null
      ? workflowBoard.artifact({
          environmentId: props.environmentId,
          input: { cardId: card.cardId, key: activeWireframeKey },
        })
      : null,
  );

  const [toggling, setToggling] = useState(false);
  const [expandedOpen, setExpandedOpen] = useState(false);
  const [showSource, setShowSource] = useState(false);
  const [contentHeight, setContentHeight] = useState<number>(360);
  const [blobPreview, setBlobPreview] = useState<{
    readonly digest: string;
    readonly url: string;
  } | null>(null);

  const htmlContent = wireframeArtifact.data?.content ?? null;
  const htmlDigest = wireframeArtifact.data?.digest ?? null;

  // oxlint-disable-next-line legioncode/no-unreasoned-state-sync -- manages Blob URL lifecycle for sandboxed wireframe HTML and revokes it on change or unmount
  useEffect(() => {
    if (htmlContent === null || htmlDigest === null) {
      // oxlint-disable-next-line react/set-state-in-effect -- clears Blob URL when wireframe artifact is absent
      setBlobPreview(null);
      return;
    }
    const url = URL.createObjectURL(
      new Blob([injectHtmlRenderBootstrap(htmlContent)], { type: "text/html" }),
    );
    setBlobPreview({ digest: htmlDigest, url });
    return () => {
      URL.revokeObjectURL(url);
    };
  }, [htmlContent, htmlDigest]);

  const handleToggleWireframes = async (enable: boolean) => {
    if (!canUpdateSpec || toggling) return;
    setToggling(true);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await updateSpec({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        wireframesIndex: enable ? DEFAULT_WIREFRAME_KEY : null,
      },
    });
    setToggling(false);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not update wireframe settings."),
      );
      return;
    }

    setExpandedOpen(false);
    props.onActionNotice(
      enable
        ? gt("Enabled optional HTML wireframes.")
        : gt("Cleared wireframes for non-UI feature."),
    );
  };

  return (
    <section
      aria-labelledby="wireframe-section-heading"
      className="flex flex-col gap-3 rounded-lg border bg-card p-4"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 id="wireframe-section-heading" className="text-sm font-semibold text-foreground">
            <T>HTML wireframes</T>
          </h3>
          {activeWireframeKey === null ? (
            <Badge variant="secondary" size="sm">
              <T>Skipped (non-UI)</T>
            </Badge>
          ) : (
            <Badge variant="outline" size="sm">
              {activeWireframeKey}
            </Badge>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {activeWireframeKey === null ? (
            <Button
              type="button"
              size="xs"
              variant="outline"
              disabled={!canUpdateSpec || toggling}
              onClick={() => void handleToggleWireframes(true)}
            >
              <T>Enable wireframes</T>
            </Button>
          ) : (
            <>
              <Button
                type="button"
                size="xs"
                variant="ghost"
                aria-expanded={showSource}
                onClick={() => setShowSource((prev) => !prev)}
              >
                {showSource ? <T>Hide HTML source</T> : <T>View HTML source</T>}
              </Button>
              {blobPreview !== null ? (
                <Button
                  type="button"
                  size="xs"
                  variant="outline"
                  onClick={() => setExpandedOpen(true)}
                >
                  <Maximize2Icon />
                  <T>Fullscreen preview</T>
                </Button>
              ) : null}
              <Button
                type="button"
                size="xs"
                variant="outline"
                disabled={!canUpdateSpec || toggling}
                onClick={() => void handleToggleWireframes(false)}
              >
                <T>Skip wireframes (non-UI)</T>
              </Button>
            </>
          )}
        </div>
      </div>

      {activeWireframeKey === null ? (
        <p className="text-xs text-muted-foreground">
          <T>
            Wireframes are optional and skipped for non-UI features. Enable them if this feature
            introduces interactive UI layouts.
          </T>
        </p>
      ) : wireframeArtifact.error !== null ? (
        <Alert variant="error">
          <AlertDescription>{wireframeArtifact.error}</AlertDescription>
        </Alert>
      ) : wireframeArtifact.data === null || blobPreview === null ? (
        <p className="text-xs text-muted-foreground" role="status">
          <T>Loading wireframe preview…</T>
        </p>
      ) : (
        <div className="flex flex-col gap-2">
          <div
            className="overflow-hidden rounded-md border bg-background"
            style={{ height: Math.min(Math.max(contentHeight, 240), 520) }}
          >
            <HtmlRenderDocument
              key={blobPreview.digest}
              src={blobPreview.url}
              title={gt("Wireframe preview for {title}", { title: card.title })}
              containScroll
              onContentHeight={setContentHeight}
              className="size-full"
            />
          </div>

          {showSource ? (
            <pre className="max-h-60 overflow-auto rounded-md border bg-muted/30 p-3 font-mono text-xs text-foreground">
              {wireframeArtifact.data.content}
            </pre>
          ) : null}

          {expandedOpen ? (
            <HtmlRenderDialog
              src={blobPreview.url}
              htmlRender={{
                attachmentId: blobPreview.digest,
                title: gt("Wireframe preview for {title}", { title: card.title }),
                height: Math.max(contentHeight, 360),
              }}
              onClose={() => setExpandedOpen(false)}
            />
          ) : null}
        </div>
      )}
    </section>
  );
}

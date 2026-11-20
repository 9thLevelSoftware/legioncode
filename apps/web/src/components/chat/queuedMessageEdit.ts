import { resolveQueuedEditExit } from "@t3tools/client-runtime/queued-run-edit";
import type {
  ChatAttachment,
  EnvironmentId,
  OrchestrationMessageContext,
} from "@t3tools/contracts";
import {
  composerDraftHasUserContent,
  useComposerDraftStore,
  type ComposerFileAttachment,
  type ComposerImageAttachment,
  type ComposerThreadTarget,
} from "../../composerDraftStore";
import { msg } from "gt-react";
import { m } from "../../i18n/messages";
import { releaseDraftAttachments } from "../../lib/attachmentUploadQueue";
import {
  asKnownContextRecord,
  previewAnnotationContextId,
  reviewCommentContextId,
  terminalContextReference,
} from "../../lib/composerContextRecords";
import {
  collectInlineContextIds,
  producerIdFromComposerContextId,
} from "../../lib/composerContextReferences";

/** Staged attachments upload as soon as they are added, so a dropped edit must release them. */
export function discardQueuedEditDraft(target: ComposerThreadTarget): void {
  const store = useComposerDraftStore.getState();
  const draft = store.getComposerDraft(target);
  if (draft) {
    releaseDraftAttachments([...draft.images, ...draft.files]);
    for (const image of draft.images) {
      if (image.previewUrl.startsWith("blob:")) URL.revokeObjectURL(image.previewUrl);
    }
  }
  store.clearComposerContent(target);
}

export function recoverQueuedMessageEdit(input: {
  readonly editTarget: ComposerThreadTarget;
  readonly threadTarget: ComposerThreadTarget;
  readonly originalText: string;
  readonly removedExistingAttachment?: boolean;
  readonly environmentId?: EnvironmentId;
  readonly existingAttachments?: ReadonlyArray<ChatAttachment>;
  readonly context?: OrchestrationMessageContext | undefined;
}): "kept" | "discarded" | "clean" {
  const store = useComposerDraftStore.getState();
  const draft = store.getComposerDraft(input.editTarget);
  const recovery = resolveQueuedEditExit({
    editChanged:
      input.removedExistingAttachment === true ||
      (draft !== null &&
        (draft.prompt !== input.originalText || draft.images.length > 0 || draft.files.length > 0)),
    // A cleared edit has nothing to put back, so it can only be discarded.
    threadDraftEmpty:
      composerDraftHasUserContent(draft) &&
      !composerDraftHasUserContent(store.getComposerDraft(input.threadTarget)),
  });
  if (recovery === "kept" && draft !== null) {
    const referencedIds = new Set(collectInlineContextIds(draft.prompt));
    const keptTerminals = draft.terminalContexts.filter((context) =>
      referencedIds.has(terminalContextReference(context).contextId),
    );
    if (keptTerminals.length !== draft.terminalContexts.length) {
      store.setTerminalContexts(input.editTarget, keptTerminals);
    }
    const keptReviewComments = draft.reviewComments.filter((comment) =>
      referencedIds.has(reviewCommentContextId(comment.id)),
    );
    if (keptReviewComments.length !== draft.reviewComments.length) {
      store.setReviewComments(input.editTarget, keptReviewComments);
    }
    const keptPreviewAnnotations = draft.previewAnnotations.filter((annotation) =>
      referencedIds.has(previewAnnotationContextId(annotation.id)),
    );
    if (keptPreviewAnnotations.length !== draft.previewAnnotations.length) {
      store.setPreviewAnnotations(input.editTarget, keptPreviewAnnotations);
    }
    const keptThreadContexts = draft.threadContexts.filter((record) =>
      referencedIds.has(record.contextId),
    );
    if (keptThreadContexts.length !== draft.threadContexts.length) {
      store.setThreadContexts(input.editTarget, keptThreadContexts);
    }
    const keptEnvironmentContexts = draft.environmentContexts.filter((record) =>
      referencedIds.has(record.contextId),
    );
    if (keptEnvironmentContexts.length !== draft.environmentContexts.length) {
      store.setEnvironmentContexts(input.editTarget, keptEnvironmentContexts);
    }
    store.moveComposerContent(input.editTarget, input.threadTarget);
    store.clearComposerContent(input.editTarget);
    if (
      input.environmentId !== undefined &&
      input.context !== undefined &&
      input.existingAttachments !== undefined
    ) {
      const remainingAttachmentIds = new Set(
        input.existingAttachments.map((attachment) => attachment.id),
      );
      const recoveredFiles: ComposerFileAttachment[] = [];
      for (const record of input.context.records) {
        const known = asKnownContextRecord(record);
        if (
          known?.kind === "file" &&
          referencedIds.has(known.contextId) &&
          remainingAttachmentIds.has(known.attachmentId)
        ) {
          recoveredFiles.push({
            type: "file",
            id: producerIdFromComposerContextId("file", known.contextId),
            name: known.name,
            mimeType: known.mimeType,
            sizeBytes: known.sizeBytes,
            file: null,
            uploadedAttachmentId: known.attachmentId,
            uploadEnvironmentId: input.environmentId,
          });
        }
      }
      if (recoveredFiles.length > 0) {
        store.addFiles(input.threadTarget, recoveredFiles);
      }
    }
  } else {
    discardQueuedEditDraft(input.editTarget);
  }
  return recovery;
}

export async function prepareQueuedEditAttachments(input: {
  readonly existingAttachments: ReadonlyArray<ChatAttachment>;
  readonly images: ReadonlyArray<ComposerImageAttachment>;
  readonly files: ReadonlyArray<ComposerFileAttachment>;
  readonly uploadFiles: (
    files: ReadonlyArray<ComposerFileAttachment>,
  ) => Promise<ReadonlyArray<ChatAttachment>>;
  readonly readImage: (file: File) => Promise<string>;
}) {
  const files = input.files.length === 0 ? [] : await input.uploadFiles(input.files);
  if (files.length !== input.files.length)
    throw new Error(m(msg("Retry or remove failed uploads before saving.")));
  const images = await Promise.all(
    input.images.map(async (image) => ({
      type: "image" as const,
      id: image.id,
      name: image.name,
      mimeType: image.mimeType,
      sizeBytes: image.sizeBytes,
      dataUrl: await input.readImage(image.file),
      ...(image.source ? { source: image.source } : {}),
    })),
  );
  return [...input.existingAttachments, ...images, ...files];
}

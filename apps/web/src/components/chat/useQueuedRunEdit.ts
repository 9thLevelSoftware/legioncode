import { msg } from "gt-react";
import { thrownErrorMessage } from "@legioncode/client-runtime/errors";
import { m } from "~/i18n/messages";
import { useAtomValue } from "@effect/atom-react";
import {
  makeQueuedRunEditStore,
  makeQueuedRunsAtom,
} from "@legioncode/client-runtime/queued-run-edit";
import type { ComposerDispatchMode } from "@legioncode/client-runtime/state/composer-dispatch";
import type { ThreadStructure } from "@legioncode/client-runtime/state/shell";
import { deriveThreadQueueWorkflowState } from "@legioncode/client-runtime/state/thread-workflows";
import {
  PROVIDER_SEND_TURN_MAX_ATTACHMENTS,
  type ChatAttachment,
  type ComposerContextId,
  type ComposerContextRecord,
  type EnvironmentContextRecord,
  type ResponseContextRecord,
  type EnvironmentId,
  type MessageId,
  type OrchestrationMessageContext,
  type PreviewAnnotationPayload,
  type RunId,
  type ScopedThreadRef,
  type ThreadContextRecord,
  type ThreadId,
} from "@legioncode/contracts";
import { scopedThreadKey } from "@legioncode/client-runtime/environment";
import {
  restoreAttachmentOnlyBootstrapPrompt,
  stripAttachmentOnlyBootstrapPrompt,
  toKindScopedComposerContextId,
} from "@legioncode/shared/composerContextReferences";
import { Atom } from "effect/reactivity";
import type { RefObject } from "react";
import { useAssetUrls } from "../../assets/assetUrls";
import {
  queuedRunEditDraftId,
  useComposerDraftStore,
  type ComposerFileAttachment,
  type ComposerImageAttachment,
  type ComposerThreadTarget,
} from "../../composerDraftStore";
import { useStableCallback } from "../../hooks/useStableCallback";
import {
  awaitAttachmentUploads,
  getUploadedAttachments,
  refreshExpiredAttachmentUploads,
  releaseDraftAttachments,
  startAttachmentUpload,
} from "../../lib/attachmentUploadQueue";
import {
  asKnownContextRecord,
  buildMessageContext,
  isResponseContextRecord,
  previewAnnotationContextId,
  previewAnnotationFromRecord,
  reviewCommentContextId,
  reviewCommentFromRecord,
  terminalContextDraftFromRecord,
  terminalContextReference,
} from "../../lib/composerContextRecords";
import {
  collectInlineContextIds,
  removeInlineContextReference,
} from "../../lib/composerContextReferences";
import { readFileAsDataUrl } from "../../lib/imageCompression";
import type { TerminalContextDraft } from "../../lib/terminalContext";
import type { ComposerSubmissionIntent } from "../../composer-logic";
import type { ReviewCommentContext } from "../../reviewCommentContext";
import { appAtomRegistry } from "../../rpc/atomRegistry";
import { environmentServerConfigsAtom } from "../../state/server";
import {
  environmentThreadDetails,
  environmentThreadShells,
  threadEnvironment,
} from "../../state/threads";
import { useAtomCommand } from "../../state/use-atom-command";
import { stackedThreadToast, toastManager } from "../ui/toast";
import type { ChatComposerHandle } from "./ChatComposer";
import { fileAttachmentCapabilityBlockReason } from "./composerAttachmentFiles";
import { composerDraftContentChanged } from "./composerSend.logic";
import {
  discardQueuedEditDraft,
  prepareQueuedEditAttachments,
  recoverQueuedMessageEdit,
} from "./queuedMessageEdit";
import type { EditQueuedRunRequest } from "./QueuedRunsControl";

interface EditingQueuedRun {
  readonly threadId: ThreadId;
  readonly runId: RunId;
  readonly messageId: MessageId;
  readonly originalText: string;
  readonly existingAttachments: ReadonlyArray<ChatAttachment>;
  readonly context?: OrchestrationMessageContext | undefined;
  /** Removing a server attachment changes the message even when the draft looks untouched. */
  readonly removedExistingAttachment?: true;
}

// Keyed by the route thread, so an edit survives navigating away and back.
const queuedRunEdits = makeQueuedRunEditStore<EditingQueuedRun>({
  registry: appAtomRegistry,
  label: "web-queued-run-edits",
});

interface QueuedRunEditContent {
  readonly prompt: string;
  readonly images: ReadonlyArray<ComposerImageAttachment>;
  readonly files: ReadonlyArray<ComposerFileAttachment>;
  readonly terminalContexts: ReadonlyArray<TerminalContextDraft>;
  readonly reviewComments: ReadonlyArray<ReviewCommentContext>;
  readonly previewAnnotations: ReadonlyArray<PreviewAnnotationPayload>;
  readonly threadContexts: ReadonlyArray<ThreadContextRecord>;
  readonly environmentContexts: ReadonlyArray<EnvironmentContextRecord>;
  readonly replyTo?: ResponseContextRecord | null;
}

const isDraftOwnedRecord = (record: ComposerContextRecord) =>
  record.kind === "thread" ||
  record.kind === "environment" ||
  record.kind === "terminal" ||
  record.kind === "review-comment" ||
  record.kind === "preview-annotation" ||
  record.kind === "response";

async function submitQueuedRunEdit(input: {
  readonly environmentId: EnvironmentId;
  readonly draftTarget: ComposerThreadTarget;
  readonly editing: EditingQueuedRun;
  readonly content: QueuedRunEditContent;
  readonly editQueuedRun: ReturnType<typeof useEditQueuedRunCommand>;
  readonly draftChanged: () => boolean;
}): Promise<string | null> {
  const { environmentId, draftTarget, editing, content } = input;
  const editText = content.prompt.trim();
  const newAttachments = [...content.images, ...content.files];
  const uploadFiles = async (files: ReadonlyArray<ComposerFileAttachment>) => {
    const validateFiles = () => {
      const config = appAtomRegistry.get(environmentServerConfigsAtom).get(environmentId) ?? null;
      const reason = fileAttachmentCapabilityBlockReason({
        files,
        attachmentUploadsCapabilityKnown: config !== null,
        supportsAttachmentUploads: config !== null,
        maxFileAttachmentBytes:
          config?.environment.capabilities.fileAttachments?.maxUploadBytes ?? null,
      });
      if (reason !== null) throw new Error(reason);
    };
    validateFiles();
    for (const file of files) startAttachmentUpload({ environmentId, image: file, draftTarget });
    await awaitAttachmentUploads(files.map((file) => file.id));
    await refreshExpiredAttachmentUploads({ environmentId, images: files, draftTarget });
    validateFiles();
    const uploaded = getUploadedAttachments({ environmentId, images: files });
    if (uploaded === null) throw new Error(m(msg("Retry or remove failed uploads before saving.")));
    return uploaded;
  };
  try {
    const uploads = await prepareQueuedEditAttachments({
      existingAttachments: editing.existingAttachments,
      images: content.images,
      files: content.files,
      readImage: readFileAsDataUrl,
      uploadFiles,
    });
    if (input.draftChanged())
      return m(msg("Draft changed while preparing this message. Review it and save again."));
    const referencedIds = new Set(collectInlineContextIds(editText));
    const originalReferencedIds = new Set(collectInlineContextIds(editing.originalText));
    const remainingAttachmentIds = new Set(
      editing.existingAttachments.map((attachment) => attachment.id),
    );
    const existingRecords = editing.context?.records ?? [];
    const existingScreenshotByAnnotationId = new Map<ComposerContextId, ComposerContextId>();
    for (const record of existingRecords) {
      const known = asKnownContextRecord(record);
      if (known?.kind !== "preview-annotation" || !known.screenshotContextId) continue;
      const screenshotRecord = existingRecords.find(
        (candidate) => candidate.contextId === known.screenshotContextId,
      );
      if (
        screenshotRecord &&
        "attachmentId" in screenshotRecord &&
        remainingAttachmentIds.has(screenshotRecord.attachmentId)
      ) {
        existingScreenshotByAnnotationId.set(known.contextId, known.screenshotContextId);
      }
    }
    const builtNewRecords =
      buildMessageContext({
        terminalContexts: content.terminalContexts.filter(
          (context) =>
            context.text.trim().length > 0 &&
            referencedIds.has(terminalContextReference(context).contextId),
        ),
        reviewComments: content.reviewComments.filter((comment) =>
          referencedIds.has(reviewCommentContextId(comment.id)),
        ),
        previewAnnotations: content.previewAnnotations.filter((annotation) =>
          referencedIds.has(previewAnnotationContextId(annotation.id)),
        ),
        threadContexts: content.threadContexts.filter((record) =>
          referencedIds.has(record.contextId),
        ),
        environmentContexts: content.environmentContexts.filter((record) =>
          referencedIds.has(record.contextId),
        ),
        replyTo: content.replyTo ?? null,
        attachments: newAttachments.map((attachment, index) => ({
          attachment,
          attachmentId: uploads[editing.existingAttachments.length + index]?.id ?? attachment.id,
        })),
      })?.records ?? [];
    const newRecords = builtNewRecords.map((record) => {
      if (
        record.kind === "preview-annotation" &&
        !("payload" in record) &&
        !record.screenshotContextId
      ) {
        const screenshotContextId = existingScreenshotByAnnotationId.get(record.contextId);
        if (screenshotContextId) {
          return {
            ...record,
            screenshotContextId,
          };
        }
      }
      return record;
    });
    const keptScreenshotContextIds = new Set(
      newRecords.flatMap((record) =>
        record.kind === "preview-annotation" && !("payload" in record) && record.screenshotContextId
          ? [record.screenshotContextId]
          : [],
      ),
    );
    const keptRecords = existingRecords.filter((record) => {
      if (isDraftOwnedRecord(record)) return false;
      if ("attachmentId" in record) {
        if (!remainingAttachmentIds.has(record.attachmentId)) return false;
        if (keptScreenshotContextIds.has(record.contextId)) return true;
        return !originalReferencedIds.has(record.contextId) || referencedIds.has(record.contextId);
      }
      return referencedIds.has(record.contextId);
    });
    const result = await input.editQueuedRun({
      environmentId,
      input: {
        threadId: editing.threadId,
        runId: editing.runId,
        text: restoreAttachmentOnlyBootstrapPrompt(editText, uploads.length > 0),
        edit: {
          messageId: editing.messageId,
          attachments: uploads,
          context: {
            version: 1,
            records: [
              ...new Map(
                [...keptRecords, ...newRecords].map(
                  (record) => [record.contextId, record] as const,
                ),
              ).values(),
            ],
          },
        },
      },
    });
    return result._tag === "Failure" ? m(msg("Could not save the edited queued message.")) : null;
  } catch (error) {
    return thrownErrorMessage(error, m) ?? m(msg("Could not save the edited queued message."));
  }
}

function useEditQueuedRunCommand() {
  return useAtomCommand(threadEnvironment.editQueuedRun, { reportFailure: false });
}

/** Edits preserve the thread draft; attachment removals stay local until save. */
export function useQueuedRunEdit(input: {
  readonly environmentId: EnvironmentId;
  readonly routeThreadRef: ScopedThreadRef;
  readonly threadDraftTarget: ComposerThreadTarget;
  readonly activeThreadId: ThreadId | null;
  readonly serverProjection: ThreadStructure | null;
  readonly promptRef: { current: string };
  readonly composerRef: RefObject<ChatComposerHandle | null>;
}) {
  const {
    environmentId,
    routeThreadRef,
    threadDraftTarget,
    activeThreadId,
    serverProjection,
    promptRef,
    composerRef,
  } = input;
  const editQueuedRunCommand = useEditQueuedRunCommand();
  const promoteQueuedRun = useAtomCommand(threadEnvironment.promoteQueuedRun);
  const editKey = scopedThreadKey(routeThreadRef);
  const storedEdit = useAtomValue(queuedRunEdits.atom)[editKey] ?? null;
  const editing = activeThreadId === null ? null : storedEdit;
  const isSaving = useAtomValue(queuedRunEdits.savingAtom)[editKey] === true;
  const draftTarget =
    editing === null ? threadDraftTarget : queuedRunEditDraftId(routeThreadRef, editing.runId);

  const imageResources = (editing?.existingAttachments ?? [])
    .filter((attachment) => attachment.type === "image")
    .map((attachment) => ({ _tag: "attachment" as const, attachmentId: attachment.id }));
  const imageUrls = useAssetUrls(environmentId, imageResources);
  const urlByAttachmentId = new Map(
    imageResources.map((resource, index) => [resource.attachmentId, imageUrls[index] ?? null]),
  );
  const editingAttachments =
    editing === null
      ? null
      : editing.existingAttachments.map((attachment) => ({
          attachment,
          url: urlByAttachmentId.get(attachment.id) ?? null,
        }));

  const focusComposerSoon = () => {
    window.requestAnimationFrame(() => composerRef.current?.focusAtEnd());
  };
  const clearEditDraft = (runId: RunId) => {
    const target = queuedRunEditDraftId(routeThreadRef, runId);
    // A save in flight still sends these uploads.
    if (isSaving) useComposerDraftStore.getState().clearComposerContent(target);
    else discardQueuedEditDraft(target);
  };

  const beginEdit = useStableCallback((request: EditQueuedRunRequest) => {
    if (activeThreadId === null) return;
    if (editing !== null && editing.runId !== request.runId) clearEditDraft(editing.runId);
    const target = queuedRunEditDraftId(routeThreadRef, request.runId);
    const sentText = stripAttachmentOnlyBootstrapPrompt(request.text);
    const context = serverProjection?.messages.find(
      (message) => message.id === request.messageId,
    )?.context;
    const referenced = new Set(collectInlineContextIds(sentText));
    const records = (context?.records ?? []).filter((record) => referenced.has(record.contextId));
    const knownRecords = records.flatMap((record) => {
      const known = asKnownContextRecord(record);
      return known ? [known] : [];
    });
    // A reply shows above the composer while its message is edited, not as a chip in the text.
    // Only the reply the text still points at: a removed or replaced one stays behind as a record.
    const reply = records.find(isResponseContextRecord);
    const text = reply ? removeInlineContextReference(sentText, reply.contextId).prompt : sentText;
    const store = useComposerDraftStore.getState();
    store.clearComposerContent(target);
    store.setPrompt(target, text);
    store.setTerminalContexts(
      target,
      knownRecords.flatMap((record) =>
        record.kind === "terminal" ? [terminalContextDraftFromRecord(record, activeThreadId)] : [],
      ),
    );
    store.setReviewComments(
      target,
      knownRecords.flatMap((record) =>
        record.kind === "review-comment" ? [reviewCommentFromRecord(record)] : [],
      ),
    );
    store.setPreviewAnnotations(
      target,
      knownRecords.flatMap((record) =>
        record.kind === "preview-annotation" ? [previewAnnotationFromRecord(record)] : [],
      ),
    );
    store.setThreadContexts(
      target,
      knownRecords.filter((record): record is ThreadContextRecord => record.kind === "thread"),
    );
    store.setEnvironmentContexts(
      target,
      knownRecords.filter(
        (record): record is EnvironmentContextRecord => record.kind === "environment",
      ),
    );
    if (reply) store.setReplyTo(target, reply);
    const originalText = store.getComposerDraft(target)?.prompt ?? text;
    const threadRef = { environmentId, threadId: activeThreadId };
    queuedRunEdits.begin(
      editKey,
      {
        threadId: activeThreadId,
        runId: request.runId,
        messageId: request.messageId,
        originalText,
        existingAttachments: request.attachments,
        context,
      },
      {
        runs: makeQueuedRunsAtom({
          threadId: activeThreadId,
          snapshot: threadEnvironment.snapshotAtom(environmentId),
          detailStatus: environmentThreadDetails.statusAtom(threadRef),
          runs: Atom.make(
            (get) =>
              get(environmentThreadDetails.quietThreadAtom(threadRef))?.projection.runs ?? null,
          ),
        }),
        // The run started or was cancelled from another client. A changed
        // edit moves into the thread's own draft when that draft is empty;
        // otherwise it is dropped. Edits outlive navigation, so the toast is
        // unscoped and names the thread: the user may be anywhere by now.
        onRunLeftQueue: (_, edit) => {
          const recovery = recoverQueuedMessageEdit({
            editTarget: queuedRunEditDraftId(routeThreadRef, edit.runId),
            threadTarget: threadDraftTarget,
            originalText: edit.originalText,
            removedExistingAttachment: edit.removedExistingAttachment === true,
            environmentId,
            existingAttachments: edit.existingAttachments,
            context: edit.context,
          });
          if (recovery === "clean") return;
          const title = appAtomRegistry.get(
            environmentThreadShells.threadShellAtom(threadRef),
          )?.title;
          toastManager.add(
            stackedThreadToast({
              type: recovery === "kept" ? "info" : "warning",
              title: m(msg("Queued message is no longer queued")),
              description:
                title === undefined
                  ? recovery === "kept"
                    ? m(msg("Your unsaved edit was kept in the composer."))
                    : m(msg("Your unsaved edit was discarded."))
                  : recovery === "kept"
                    ? m(msg("Your unsaved edit in “{title}” was kept in the composer.", { title }))
                    : m(msg("Your unsaved edit in “{title}” was discarded.", { title })),
            }),
          );
        },
      },
    );
    focusComposerSoon();
  });
  const cancelEdit = useStableCallback(() => {
    if (editing === null) return;
    clearEditDraft(editing.runId);
    queuedRunEdits.end(editKey);
    focusComposerSoon();
  });
  const removeEditingAttachment = useStableCallback((attachmentId: string) => {
    const current = queuedRunEdits.get(editKey);
    if (current !== null) {
      const removedAttachment = current.existingAttachments.find(
        (attachment) => attachment.id === attachmentId,
      );
      const contextIds = new Set<string>();
      for (const record of current.context?.records ?? []) {
        if ("attachmentId" in record && record.attachmentId === attachmentId) {
          contextIds.add(record.contextId);
        }
      }
      if (removedAttachment?.type === "image" || removedAttachment?.type === "file") {
        contextIds.add(toKindScopedComposerContextId(removedAttachment.type, attachmentId));
      }
      if (contextIds.size > 0) {
        const target = queuedRunEditDraftId(routeThreadRef, current.runId);
        const store = useComposerDraftStore.getState();
        const draft = store.getComposerDraft(target);
        if (draft !== null) {
          let prompt = draft.prompt;
          for (const contextId of contextIds) {
            prompt = removeInlineContextReference(prompt, contextId).prompt;
          }
          if (prompt !== draft.prompt) {
            store.setPrompt(target, prompt);
          }
        }
      }
    }
    queuedRunEdits.update(editKey, (currentEdit) => ({
      ...currentEdit,
      removedExistingAttachment: true,
      existingAttachments: currentEdit.existingAttachments.filter(
        (attachment) => attachment.id !== attachmentId,
      ),
    }));
  });

  // The same move as the queued row's "Send now" while a turn runs.
  const steerSavedRun = async (edited: EditingQueuedRun) => {
    const projection = appAtomRegistry.get(
      environmentThreadDetails.quietThreadAtom({ environmentId, threadId: edited.threadId }),
    )?.projection;
    if (!projection) return;
    const workflow = deriveThreadQueueWorkflowState(projection);
    const queued = workflow.queuedRuns.find(({ run }) => run.id === edited.runId);
    if (
      queued === undefined ||
      workflow.activeRun === null ||
      !workflow.canPromoteToSteer ||
      (queued.run.freshContext === true && !workflow.promoteSkipsCompaction)
    ) {
      return;
    }
    await promoteQueuedRun({
      environmentId,
      input: {
        threadId: edited.threadId,
        queuedRunId: edited.runId,
        targetRunId: workflow.activeRun.id,
      },
    });
  };

  const saveEdit = async (
    content: QueuedRunEditContent,
    setThreadError: (threadId: ThreadId, error: string | null) => void,
    submission?: {
      readonly dispatchMode: ComposerDispatchMode;
      readonly submissionIntent: ComposerSubmissionIntent;
    },
  ) => {
    if (editing === null || isSaving) return;
    const newAttachments = [...content.images, ...content.files];
    if (
      editing.existingAttachments.length + newAttachments.length >
      PROVIDER_SEND_TURN_MAX_ATTACHMENTS
    ) {
      setThreadError(
        editing.threadId,
        m(
          msg("A message can have at most {max} attachments.", {
            max: PROVIDER_SEND_TURN_MAX_ATTACHMENTS,
          }),
        ),
      );
      return;
    }
    if (
      content.prompt.trim().length === 0 &&
      editing.existingAttachments.length === 0 &&
      newAttachments.length === 0
    ) {
      return;
    }
    // The save's result decides the edit's fate, even if the run starts meanwhile.
    const finishSave = queuedRunEdits.startSave(editKey);
    if (finishSave === null) return;
    const draftBeforeSave = useComposerDraftStore.getState().getComposerDraft(draftTarget);
    const draftChanged = () =>
      appAtomRegistry.get(queuedRunEdits.atom)[editKey] !== editing ||
      composerDraftContentChanged(
        draftBeforeSave,
        useComposerDraftStore.getState().getComposerDraft(draftTarget),
      );
    const error = await submitQueuedRunEdit({
      environmentId,
      draftTarget,
      editing,
      content,
      editQueuedRun: editQueuedRunCommand,
      draftChanged,
    });
    const changedAfterSave = error === null && draftChanged();
    setThreadError(
      editing.threadId,
      changedAfterSave
        ? m(msg("The earlier edit was saved. Review your newer changes and save again."))
        : error,
    );
    const current = finishSave(error === null && !changedAfterSave);
    if (!current) {
      // A failed save can recover into the thread draft; a newer edit can also retain resources.
      const drafts = Object.values(useComposerDraftStore.getState().draftsByThreadKey);
      const retainedIds = new Set(
        drafts.flatMap((draft) =>
          [...draft.images, ...draft.files].map((attachment) => attachment.id),
        ),
      );
      releaseDraftAttachments(
        newAttachments.filter((attachment) => !retainedIds.has(attachment.id)),
      );
      const retainedPreviews = new Set(
        drafts.flatMap((draft) => draft.images.map((image) => image.previewUrl)),
      );
      for (const image of content.images) {
        if (image.previewUrl.startsWith("blob:") && !retainedPreviews.has(image.previewUrl)) {
          URL.revokeObjectURL(image.previewUrl);
        }
      }
      return;
    }
    if (error !== null) return;
    if (changedAfterSave) return;
    releaseDraftAttachments(newAttachments);
    promptRef.current = "";
    clearEditDraft(editing.runId);
    composerRef.current?.resetCursorState();
    focusComposerSoon();
    // A plain submit means "Update queued message"; only the opposite-action shortcut or
    // modifier-click asks to steer, as it does for a new follow-up.
    if (submission?.submissionIntent === "alternate" && submission.dispatchMode === "steer") {
      await steerSavedRun(editing);
    }
  };

  return {
    editingRunId: editing?.runId ?? null,
    isEditing: editing !== null,
    isSaving,
    draftTarget,
    editingAttachments,
    beginEdit,
    cancelEdit,
    removeEditingAttachment,
    saveEdit,
  };
}

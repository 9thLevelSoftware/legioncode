import type { ThreadStructure } from "@t3tools/client-runtime/state/shell";
import {
  EnvironmentId,
  ChatAttachmentId,
  MessageId,
  RunId,
  ThreadId,
  TurnItemId,
  type PreviewAnnotationPayload,
  type ScopedThreadRef,
} from "@t3tools/contracts";
import { useImperativeHandle, type RefObject } from "react";
import { describe, expect, it, vi } from "vite-plus/test";
import {
  ATTACHMENT_ONLY_BOOTSTRAP_PROMPT,
  threadContextRecord,
  toKindScopedComposerContextId,
} from "@t3tools/shared/composerContextReferences";
import { toastManager } from "../ui/toast";

import { act, render } from "../../../test/render";
import { useComposerDraftStore, useComposerThreadDraft } from "../../composerDraftStore";
import {
  formatInlineContextReference,
  removeInlineContextReference,
} from "../../lib/composerContextReferences";
import { INLINE_TERMINAL_CONTEXT_PLACEHOLDER } from "../../lib/terminalContext";
import {
  environmentContextRecord,
  environmentContextReference,
  previewAnnotationContextRecord,
  previewAnnotationContextReference,
  responseContextRecord,
  responseContextReference,
  reviewCommentContextRecord,
  reviewCommentContextReference,
  terminalContextRecord,
  terminalContextReference,
  threadContextReference,
} from "../../lib/composerContextRecords";
import type { ReviewCommentContext } from "../../reviewCommentContext";
import { deriveComposerSendState } from "../ChatView.logic";
import { ComposerPrimaryActions } from "./ComposerPrimaryActions";
import { AppAtomRegistryProvider, appAtomRegistry } from "../../rpc/atomRegistry";
import { useQueuedRunEdit } from "./useQueuedRunEdit";
import { readAttachmentUpload, useAttachmentUploadStore } from "../../lib/attachmentUploadQueue";

interface RunStatus {
  readonly id: RunId;
  readonly status: string;
  readonly activeAttemptId?: string;
}

const mocks = await vi.hoisted(async () => {
  const { Atom } = await import("effect/reactivity");
  return {
    editQueuedRun: vi.fn<(value: unknown) => Promise<{ readonly _tag: "Success" | "Failure" }>>(),
    promoteQueuedRun: vi.fn<(value: unknown) => Promise<{ readonly _tag: "Success" }>>(),
    promoteCommand: {},
    // The projection's runs per thread, written by each test to stand in for the server.
    threadRuns: Atom.family((_key: string) =>
      Atom.make<ReadonlyArray<RunStatus> | null>(null).pipe(Atom.keepAlive),
    ),
  };
});

vi.mock("../../state/use-atom-command", () => ({
  useAtomCommand: (command: unknown) =>
    command === mocks.promoteCommand ? mocks.promoteQueuedRun : mocks.editQueuedRun,
}));

vi.mock("../../hooks/useSettings", () => ({
  useEnvironmentIdentificationMode: () => "none",
}));

const runsKey = (ref: ScopedThreadRef) => `${ref.environmentId}:${ref.threadId}`;

vi.mock("../../state/threads", async () => {
  const { Atom } = await import("effect/reactivity");
  const key = (ref: ScopedThreadRef) => `${ref.environmentId}:${ref.threadId}`;
  return {
    threadEnvironment: {
      snapshotAtom: () => Atom.make(null),
      editQueuedRun: {},
      promoteQueuedRun: mocks.promoteCommand,
    },
    environmentThreadDetails: {
      statusAtom: () => Atom.make("ready"),
      quietThreadAtom: (ref: ScopedThreadRef) =>
        Atom.make((get) => {
          const runs = get(mocks.threadRuns(key(ref)));
          if (runs === null) return null;
          // A Claude-like session whose running turn takes steering.
          return {
            projection: {
              thread: { id: ref.threadId, activeProviderThreadId: null },
              runs,
              messages: [],
              providerThreads: [],
              providerSessions: [
                {
                  id: "session",
                  status: "running",
                  capabilities: { turns: { supportsActiveSteering: true } },
                },
              ],
              providerTurns: runs.flatMap((run) =>
                run.activeAttemptId
                  ? [{ runAttemptId: run.activeAttemptId, status: "running" }]
                  : [],
              ),
            },
          };
        }),
    },
    environmentThreadShells: {
      threadShellAtom: () => Atom.make(null),
    },
  };
});

const ENV = EnvironmentId.make("env-1");
const THREAD_A: ScopedThreadRef = { environmentId: ENV, threadId: ThreadId.make("thread-a") };
const THREAD_B: ScopedThreadRef = { environmentId: ENV, threadId: ThreadId.make("thread-b") };
const RUN = RunId.make("run-1");

type HookResult = ReturnType<typeof useQueuedRunEdit>;

function projection(runs: ReadonlyArray<RunStatus>, messages: ReadonlyArray<unknown>) {
  return { runs, messages } as unknown as ThreadStructure;
}

function setRuns(ref: ScopedThreadRef, status: string) {
  appAtomRegistry.set(mocks.threadRuns(runsKey(ref)), [{ id: RUN, status }]);
}

function Harness(props: {
  readonly threadRef: ScopedThreadRef;
  readonly runStatus: string;
  readonly hookRef: RefObject<HookResult | null>;
  readonly messages: ReadonlyArray<unknown>;
}) {
  const result = useQueuedRunEdit({
    environmentId: ENV,
    routeThreadRef: props.threadRef,
    threadDraftTarget: props.threadRef,
    activeThreadId: props.threadRef.threadId,
    serverProjection: projection([{ id: RUN, status: props.runStatus }], props.messages),
    promptRef: { current: "" },
    composerRef: { current: null },
  });
  useImperativeHandle(props.hookRef, () => result);
  const draft = useComposerThreadDraft(result.draftTarget);
  const sendState = deriveComposerSendState({
    prompt: draft.prompt,
    imageCount: draft.images.length + draft.files.length,
    existingAttachmentCount: result.editingAttachments?.length ?? 0,
    terminalContexts: draft.terminalContexts,
  });
  return result.isEditing ? (
    <ComposerPrimaryActions
      compact={false}
      canOperateThread
      pendingAction={null}
      isRunning={false}
      canInterrupt={false}
      isSendBusy={result.isSaving}
      sendDisabledReason={null}
      isEnvironmentUnavailable={false}
      isPreparingWorktree={false}
      hasSendableContent={sendState.hasSendableContent}
      isEditingQueuedMessage
      onSubmitMessage={() => void result.saveEdit(savedContent(draft.prompt), () => {})}
      onPreviousPendingQuestion={() => {}}
      onInterrupt={() => {}}
    />
  ) : null;
}

function view(
  threadRef: ScopedThreadRef,
  runStatus: string,
  hook: { current: HookResult | null },
  messages: ReadonlyArray<unknown> = [],
) {
  return (
    <AppAtomRegistryProvider>
      <Harness threadRef={threadRef} runStatus={runStatus} hookRef={hook} messages={messages} />
    </AppAtomRegistryProvider>
  );
}

function setup() {
  mocks.editQueuedRun.mockReset();
  mocks.promoteQueuedRun.mockReset();
  useAttachmentUploadStore.setState({ uploadsByImageId: {} });
  useComposerDraftStore.setState({ draftsByThreadKey: {}, draftThreadsByThreadKey: {} });
  setRuns(THREAD_A, "queued");
  setRuns(THREAD_B, "queued");
  return { hook: { current: null as HookResult | null } };
}

function requireHook(hook: { current: HookResult | null }): HookResult {
  if (hook.current === null) throw new Error("hook did not render");
  return hook.current;
}

async function beginChangedEdit(hook: { current: HookResult | null }) {
  await act(() =>
    requireHook(hook).beginEdit({
      runId: RUN,
      messageId: MessageId.make("message-1"),
      text: "original",
      attachments: [],
    }),
  );
  await act(() =>
    useComposerDraftStore.getState().setPrompt(requireHook(hook).draftTarget, "changed"),
  );
}

const savedContent = (prompt: string) => ({
  prompt,
  images: [],
  files: [],
  terminalContexts: [],
  reviewComments: [],
  previewAnnotations: [],
  threadContexts: [],
  environmentContexts: [],
});

const promptOf = (ref: ScopedThreadRef) =>
  useComposerDraftStore.getState().getComposerDraft(ref)?.prompt ?? "";

describe("useQueuedRunEdit", () => {
  it.each(["", "Ultrathink:\n"])(
    "opens attachment-only edits without the placeholder and preserves effort on save (%j)",
    async (prefix) => {
      const { hook } = setup();
      mocks.editQueuedRun.mockResolvedValue({ _tag: "Success" });
      await using screen = await render(view(THREAD_A, "queued", hook));
      using toast = vi.spyOn(toastManager, "add");
      const attachments = [
        {
          type: "image" as const,
          id: ChatAttachmentId.make("kept"),
          name: "kept.png",
          mimeType: "image/png",
          sizeBytes: 10,
        },
      ];
      const request = {
        runId: RUN,
        messageId: MessageId.make("message-1"),
        text: prefix + ATTACHMENT_ONLY_BOOTSTRAP_PROMPT,
        attachments,
      };
      await act(() => requireHook(hook).beginEdit(request));
      expect(
        useComposerDraftStore.getState().getComposerDraft(requireHook(hook).draftTarget)?.prompt ??
          "",
      ).toBe(prefix.trimEnd());
      expect(requireHook(hook).editingAttachments?.map(({ attachment }) => attachment)).toEqual(
        attachments,
      );
      await act(() =>
        useComposerDraftStore
          .getState()
          .setRuntimeMode(requireHook(hook).draftTarget, "full-access"),
      );
      await act(() => setRuns(THREAD_A, "running"));
      expect(toast).not.toHaveBeenCalled();
      expect(requireHook(hook).isEditing).toBe(false);
      await act(() => setRuns(THREAD_A, "queued"));
      await act(() => requireHook(hook).beginEdit(request));
      const update = screen.getByRole("button", { name: "Update queued message" });
      await expect.element(update).toBeEnabled();
      await update.click();
      await vi.waitFor(() => expect(mocks.editQueuedRun).toHaveBeenCalledOnce());
      expect(mocks.editQueuedRun).toHaveBeenCalledWith(
        expect.objectContaining({
          input: expect.objectContaining({
            text: prefix + ATTACHMENT_ONLY_BOOTSTRAP_PROMPT,
            edit: expect.objectContaining({ attachments }),
          }),
        }),
      );
    },
  );

  it.each([
    // Cmd+Enter or Cmd-click with Queue as the default: save, then steer.
    { dispatchMode: "steer", submissionIntent: "alternate", steers: true },
    // Enter or the Update button with Steer as the default: save in place only.
    { dispatchMode: "steer", submissionIntent: "foreground", steers: false },
    // Cmd+Enter with Steer as the default queues, so the message stays queued.
    { dispatchMode: "queue", submissionIntent: "alternate", steers: false },
  ] as const)(
    "a saved edit sent as $submissionIntent/$dispatchMode steers: $steers",
    async (submission) => {
      const { hook } = setup();
      mocks.editQueuedRun.mockResolvedValue({ _tag: "Success" });
      mocks.promoteQueuedRun.mockResolvedValue({ _tag: "Success" });
      const activeRun = RunId.make("run-active");
      appAtomRegistry.set(mocks.threadRuns(runsKey(THREAD_A)), [
        { id: activeRun, status: "running", activeAttemptId: "attempt-1" },
        { id: RUN, status: "queued" },
      ]);
      await using _screen = await render(view(THREAD_A, "queued", hook));
      await beginChangedEdit(hook);
      await act(() => requireHook(hook).saveEdit(savedContent("changed"), () => {}, submission));
      expect(mocks.editQueuedRun).toHaveBeenCalledOnce();
      if (!submission.steers) {
        expect(mocks.promoteQueuedRun).not.toHaveBeenCalled();
        return;
      }
      expect(mocks.promoteQueuedRun).toHaveBeenCalledExactlyOnceWith({
        environmentId: ENV,
        input: { threadId: THREAD_A.threadId, queuedRunId: RUN, targetRunId: activeRun },
      });
      expect(mocks.editQueuedRun.mock.invocationCallOrder[0]).toBeLessThan(
        mocks.promoteQueuedRun.mock.invocationCallOrder[0]!,
      );
    },
  );

  it("saves an effort-only prompt without claiming removed attachments remain", async () => {
    const { hook } = setup();
    mocks.editQueuedRun.mockResolvedValue({ _tag: "Success" });
    await using screen = await render(view(THREAD_A, "queued", hook));
    await act(() =>
      requireHook(hook).beginEdit({
        runId: RUN,
        messageId: MessageId.make("message-1"),
        text: `Ultrathink:\n${ATTACHMENT_ONLY_BOOTSTRAP_PROMPT}`,
        attachments: [
          {
            type: "image",
            id: ChatAttachmentId.make("removed"),
            name: "removed.png",
            mimeType: "image/png",
            sizeBytes: 10,
          },
        ],
      }),
    );
    await act(() => requireHook(hook).removeEditingAttachment("removed"));
    await screen.getByRole("button", { name: "Update queued message" }).click();
    await vi.waitFor(() => expect(mocks.editQueuedRun).toHaveBeenCalledOnce());
    expect(mocks.editQueuedRun).toHaveBeenCalledWith(
      expect.objectContaining({
        input: expect.objectContaining({
          text: "Ultrathink:",
          edit: expect.objectContaining({ attachments: [] }),
        }),
      }),
    );
  });

  it("opens an edit with only the thread and machine records its text still mentions", async () => {
    const { hook } = setup();
    const thread = threadContextRecord(THREAD_B, "Support Any File Uploads");
    const machine = environmentContextRecord(ENV, "Build box");
    const unreferenced = threadContextRecord(THREAD_A, "Removed earlier");
    const messageId = MessageId.make("message-1");
    const text = `wait for ${formatInlineContextReference(threadContextReference(thread))} on ${formatInlineContextReference(environmentContextReference(machine))}`;
    const messages = [
      { id: messageId, text, context: { version: 1, records: [thread, machine, unreferenced] } },
    ];
    await using _screen = await render(view(THREAD_A, "queued", hook, messages));

    await act(() => requireHook(hook).beginEdit({ runId: RUN, messageId, text, attachments: [] }));

    const draft = useComposerDraftStore.getState().getComposerDraft(requireHook(hook).draftTarget);
    expect(draft?.prompt).toBe(text);
    expect(draft?.threadContexts).toEqual([thread]);
    expect(draft?.environmentContexts).toEqual([machine]);
  });

  it("saves only the thread mentions still in the edited message", async () => {
    const { hook } = setup();
    mocks.editQueuedRun.mockResolvedValue({ _tag: "Success" });
    const kept = threadContextRecord(THREAD_B, "Kept");
    const removed = threadContextRecord(THREAD_A, "Removed");
    const messageId = MessageId.make("message-1");
    const text = [kept, removed]
      .map((record) => formatInlineContextReference(threadContextReference(record)))
      .join(" and ");
    const messages = [{ id: messageId, text, context: { version: 1, records: [kept, removed] } }];
    await using _screen = await render(view(THREAD_A, "queued", hook, messages));
    await act(() => requireHook(hook).beginEdit({ runId: RUN, messageId, text, attachments: [] }));
    const target = requireHook(hook).draftTarget;
    await act(() => useComposerDraftStore.getState().setThreadContexts(target, [kept]));

    const draft = useComposerDraftStore.getState().getComposerDraft(target);
    await requireHook(hook).saveEdit(
      { ...savedContent(draft?.prompt ?? ""), threadContexts: draft?.threadContexts ?? [] },
      () => {},
    );

    expect(mocks.editQueuedRun).toHaveBeenCalledWith(
      expect.objectContaining({
        input: expect.objectContaining({
          edit: expect.objectContaining({ context: { version: 1, records: [kept] } }),
        }),
      }),
    );
  });

  it("adds terminal context to the queued message being edited", async () => {
    const { hook } = setup();
    await using _screen = await render(view(THREAD_A, "queued", hook));
    await beginChangedEdit(hook);
    const target = requireHook(hook).draftTarget;
    const inserted = useComposerDraftStore.getState().insertTerminalContext(
      target,
      `changed${INLINE_TERMINAL_CONTEXT_PLACEHOLDER}`,
      {
        id: "terminal-context-1",
        threadId: THREAD_A.threadId,
        createdAt: "2026-01-01T00:00:00.000Z",
        terminalId: "default",
        terminalLabel: "Terminal 1",
        lineStart: 3,
        lineEnd: 3,
        text: "hello",
      },
      0,
    );
    expect(inserted).toBe(true);
    expect(
      useComposerDraftStore.getState().getComposerDraft(target)?.terminalContexts,
    ).toHaveLength(1);
  });

  it("keeps an open edit when the user leaves the thread and comes back", async () => {
    const { hook } = setup();
    await using screen = await render(view(THREAD_A, "queued", hook));
    await beginChangedEdit(hook);

    await screen.rerender(view(THREAD_B, "queued", hook));
    expect(requireHook(hook).isEditing).toBe(false);
    await screen.rerender(view(THREAD_A, "queued", hook));

    expect(requireHook(hook).editingRunId).toBe(RUN);
    expect(
      useComposerDraftStore.getState().getComposerDraft(requireHook(hook).draftTarget)?.prompt,
    ).toBe("changed");
  });

  it("moves a changed edit into the thread draft when its run starts while the user is away", async () => {
    const { hook } = setup();
    await using screen = await render(view(THREAD_A, "queued", hook));
    await beginChangedEdit(hook);

    await screen.rerender(view(THREAD_B, "queued", hook));
    expect(promptOf(THREAD_A)).toBe("");
    await act(() => setRuns(THREAD_A, "running"));

    expect(promptOf(THREAD_A)).toBe("changed");
  });

  it("does not copy a saved edit into the thread draft when the run starts mid-save", async () => {
    const { hook } = setup();
    let finishSave: (result: { readonly _tag: "Success" }) => void = () => {};
    mocks.editQueuedRun.mockReturnValue(
      new Promise((resolve) => {
        finishSave = resolve;
      }),
    );
    await using screen = await render(view(THREAD_A, "queued", hook));
    await beginChangedEdit(hook);

    // Not inside act(): act would hold the projection update until the save settles.
    const saved = requireHook(hook).saveEdit(savedContent("changed"), () => {});
    await vi.waitFor(() => expect(mocks.editQueuedRun).toHaveBeenCalledOnce());
    // The server applied the edit and then started the run; the projection
    // reports the start before the edit's acknowledgement arrives.
    await act(() => setRuns(THREAD_A, "running"));
    await screen.rerender(view(THREAD_A, "running", hook));
    finishSave({ _tag: "Success" });
    await act(() => saved);

    expect(requireHook(hook).isEditing).toBe(false);
    expect(promptOf(THREAD_A)).toBe("");
  });

  it.each([
    { transition: "cancel", result: "Success" },
    { transition: "recover", result: "Failure" },
    { transition: "replace", result: "Success" },
  ] as const)(
    "settles saved attachments after $transition and $result",
    async ({ transition, result }) => {
      const { hook } = setup();
      let finishSave!: (result: { readonly _tag: "Success" | "Failure" }) => void;
      let started!: () => void;
      const saving = new Promise<{ readonly _tag: "Success" | "Failure" }>((resolve) => {
        finishSave = resolve;
      });
      const requested = new Promise<void>((resolve) => {
        started = resolve;
      });
      mocks.editQueuedRun.mockImplementation(() => {
        started();
        return saving;
      });
      await using _screen = await render(view(THREAD_A, "queued", hook));
      await beginChangedEdit(hook);
      const image = {
        type: "image" as const,
        id: "saved-image",
        name: "image.png",
        mimeType: "image/png",
        sizeBytes: 5,
        file: new File(["image"], "image.png", { type: "image/png" }),
        previewUrl: "blob:saved-image",
      };
      using revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
      await act(() =>
        useComposerDraftStore.getState().addImage(requireHook(hook).draftTarget, image),
      );
      useAttachmentUploadStore.setState({
        uploadsByImageId: {
          [image.id]: { status: "ready", environmentId: ENV, attachmentId: "saved-upload" },
        },
      });
      const target = requireHook(hook).draftTarget;
      const saved = requireHook(hook).saveEdit(
        { ...savedContent("changed"), images: [image] },
        () => {},
      );
      await act(() => requested);
      expect(requireHook(hook).isSaving).toBe(true);
      if (transition === "cancel") await act(() => requireHook(hook).cancelEdit());
      if (transition === "recover") await act(() => setRuns(THREAD_A, "running"));
      if (transition === "replace") {
        await beginChangedEdit(hook);
        await act(() =>
          useComposerDraftStore.getState().addImage(requireHook(hook).draftTarget, image),
        );
      }
      expect(readAttachmentUpload(image.id)).toBeDefined();
      expect(revoke).not.toHaveBeenCalled();
      finishSave({ _tag: result });
      await act(() => saved);
      if (transition === "cancel") {
        expect(readAttachmentUpload(image.id)).toBeUndefined();
        expect(revoke).toHaveBeenCalledWith(image.previewUrl);
        expect(useComposerDraftStore.getState().getComposerDraft(target)).toBeNull();
      } else {
        expect(readAttachmentUpload(image.id)).toBeDefined();
        expect(revoke).not.toHaveBeenCalled();
        const owner = transition === "recover" ? THREAD_A : target;
        expect(useComposerDraftStore.getState().getComposerDraft(owner)?.images).toEqual([image]);
      }
    },
  );

  it("leaves a newer edit alone when a save from before navigation finishes", async () => {
    const { hook } = setup();
    let finishSave: (result: { readonly _tag: "Success" }) => void = () => {};
    mocks.editQueuedRun.mockReturnValue(
      new Promise((resolve) => {
        finishSave = resolve;
      }),
    );
    await using screen = await render(view(THREAD_A, "queued", hook));
    await beginChangedEdit(hook);
    // Not inside act(): act would hold the rerenders until the save settles.
    const saved = requireHook(hook).saveEdit(savedContent("changed"), () => {});
    await vi.waitFor(() => expect(mocks.editQueuedRun).toHaveBeenCalledOnce());

    // Leaving the thread unmounts the chat view; coming back mounts a new one.
    await screen.rerender(<AppAtomRegistryProvider>{null}</AppAtomRegistryProvider>);
    await screen.rerender(view(THREAD_A, "queued", hook));
    expect(requireHook(hook).isSaving).toBe(true);
    await beginChangedEdit(hook);
    await act(() =>
      useComposerDraftStore.getState().setPrompt(requireHook(hook).draftTarget, "newer"),
    );

    finishSave({ _tag: "Success" });
    await act(() => saved);

    expect(requireHook(hook).editingRunId).toBe(RUN);
    expect(requireHook(hook).isSaving).toBe(false);
    expect(
      useComposerDraftStore.getState().getComposerDraft(requireHook(hook).draftTarget)?.prompt,
    ).toBe("newer");
  });

  it("hydrates all inline context records on beginEdit and prunes removed inline records on save", async () => {
    const { hook } = setup();
    mocks.editQueuedRun.mockResolvedValue({ _tag: "Success" });
    const messageId = MessageId.make("message-1");
    const terminalDraft = {
      id: "term-ctx-1",
      threadId: THREAD_A.threadId,
      createdAt: "2026-01-01T00:00:00.000Z",
      terminalId: "default",
      terminalLabel: "Terminal 1",
      lineStart: 1,
      lineEnd: 3,
      text: "pnpm test failed",
    };
    const reviewComment: ReviewCommentContext = {
      id: "review-ctx-1",
      sectionId: "diff:src/app.ts",
      sectionTitle: "src/app.ts",
      filePath: "src/app.ts",
      startIndex: 0,
      endIndex: 1,
      rangeLabel: "+1 to +2",
      text: "Handle null case",
      diff: "+return value;",
    };
    const previewAnnotation: PreviewAnnotationPayload = {
      id: "preview-ctx-1",
      pageUrl: "http://localhost:3000",
      pageTitle: "Home",
      comment: "Align header",
      elements: [],
      regions: [],
      strokes: [],
      styleChanges: [],
      screenshot: null,
      createdAt: "2026-01-01T00:00:00.000Z",
    };
    const thread = threadContextRecord(THREAD_B, "Thread B");
    const machine = environmentContextRecord(ENV, "Build box");
    const reply = responseContextRecord({
      citation: {
        version: 1,
        environmentId: ENV,
        threadId: THREAD_A.threadId,
        messageId: MessageId.make("assistant-1"),
        text: "Earlier response",
        start: 0,
        end: 16,
        prefix: "",
        suffix: "",
      },
      itemId: TurnItemId.make("item-1"),
      subagents: [],
    });
    const shotAttachment = {
      type: "image" as const,
      id: ChatAttachmentId.make("att-shot-1"),
      name: "preview-annotation-preview-ctx-1.png",
      mimeType: "image/png",
      sizeBytes: 12,
    };
    const inlineImageAttachment = {
      type: "image" as const,
      id: ChatAttachmentId.make("att-img-1"),
      name: "shot.png",
      mimeType: "image/png",
      sizeBytes: 15,
    };
    const inlineFileAttachment = {
      type: "file" as const,
      id: ChatAttachmentId.make("att-file-1"),
      name: "notes.txt",
      mimeType: "text/plain",
      sizeBytes: 20,
    };
    const previewRecord = previewAnnotationContextRecord(previewAnnotation, {
      screenshotContextId: previewAnnotation.id,
    });
    const shotImageRecord = {
      version: 1 as const,
      kind: "image" as const,
      contextId: toKindScopedComposerContextId("image", previewAnnotation.id),
      label: shotAttachment.name,
      attachmentId: shotAttachment.id,
      name: shotAttachment.name,
      mimeType: shotAttachment.mimeType,
      sizeBytes: shotAttachment.sizeBytes,
    };
    const inlineImageRecord = {
      version: 1 as const,
      kind: "image" as const,
      contextId: toKindScopedComposerContextId("image", "inline-img"),
      label: inlineImageAttachment.name,
      attachmentId: inlineImageAttachment.id,
      name: inlineImageAttachment.name,
      mimeType: inlineImageAttachment.mimeType,
      sizeBytes: inlineImageAttachment.sizeBytes,
    };
    const inlineFileRecord = {
      version: 1 as const,
      kind: "file" as const,
      contextId: toKindScopedComposerContextId("file", "inline-file"),
      label: inlineFileAttachment.name,
      attachmentId: inlineFileAttachment.id,
      name: inlineFileAttachment.name,
      mimeType: inlineFileAttachment.mimeType,
      sizeBytes: inlineFileAttachment.sizeBytes,
    };
    const terminalRecord = terminalContextRecord(terminalDraft);
    const reviewRecord = reviewCommentContextRecord(reviewComment);
    const inlineFileToken = formatInlineContextReference({
      kind: "file",
      contextId: inlineFileRecord.contextId,
      label: inlineFileRecord.label,
    });
    const inlineImageToken = formatInlineContextReference({
      kind: "image",
      contextId: inlineImageRecord.contextId,
      label: inlineImageRecord.label,
    });
    const sentText = [
      formatInlineContextReference(responseContextReference(reply)),
      formatInlineContextReference(terminalContextReference(terminalDraft)),
      formatInlineContextReference(reviewCommentContextReference(reviewComment)),
      formatInlineContextReference(previewAnnotationContextReference(previewAnnotation)),
      formatInlineContextReference(threadContextReference(thread)),
      formatInlineContextReference(environmentContextReference(machine)),
      inlineImageToken,
      inlineFileToken,
    ].join(" ");
    const messages = [
      {
        id: messageId,
        text: sentText,
        context: {
          version: 1,
          records: [
            reply,
            terminalRecord,
            reviewRecord,
            previewRecord,
            shotImageRecord,
            thread,
            machine,
            inlineImageRecord,
            inlineFileRecord,
          ],
        },
      },
    ];
    await using _screen = await render(view(THREAD_A, "queued", hook, messages));
    await act(() =>
      requireHook(hook).beginEdit({
        runId: RUN,
        messageId,
        text: sentText,
        attachments: [shotAttachment, inlineImageAttachment, inlineFileAttachment],
      }),
    );

    const target = requireHook(hook).draftTarget;
    const hydrated = useComposerDraftStore.getState().getComposerDraft(target);
    expect(hydrated?.terminalContexts).toHaveLength(1);
    expect(hydrated?.terminalContexts[0]).toMatchObject({
      terminalId: terminalDraft.terminalId,
      text: terminalDraft.text,
    });
    expect(hydrated?.reviewComments).toHaveLength(1);
    expect(hydrated?.reviewComments[0]).toMatchObject({
      filePath: reviewComment.filePath,
      text: reviewComment.text,
    });
    expect(hydrated?.previewAnnotations).toHaveLength(1);
    expect(hydrated?.previewAnnotations[0]).toMatchObject({
      id: previewAnnotation.id,
      comment: previewAnnotation.comment,
    });
    expect(hydrated?.threadContexts).toEqual([thread]);
    expect(hydrated?.environmentContexts).toEqual([machine]);
    expect(hydrated?.replyTo).toEqual(reply);

    // Delete terminal, review-comment, and inline file tokens from prompt without calling setters,
    // and clear replyTo; save must not retain their records.
    let nextPrompt = removeInlineContextReference(
      hydrated!.prompt,
      terminalRecord.contextId,
    ).prompt;
    nextPrompt = removeInlineContextReference(nextPrompt, reviewRecord.contextId).prompt;
    nextPrompt = removeInlineContextReference(nextPrompt, inlineFileRecord.contextId).prompt;
    await act(() => {
      const store = useComposerDraftStore.getState();
      store.setPrompt(target, nextPrompt);
      store.setReplyTo(target, null);
    });
    const editedDraft = useComposerDraftStore.getState().getComposerDraft(target)!;
    await requireHook(hook).saveEdit(
      {
        prompt: editedDraft.prompt,
        images: editedDraft.images,
        files: editedDraft.files,
        terminalContexts: editedDraft.terminalContexts,
        reviewComments: editedDraft.reviewComments,
        previewAnnotations: editedDraft.previewAnnotations,
        threadContexts: editedDraft.threadContexts,
        environmentContexts: editedDraft.environmentContexts,
        replyTo: editedDraft.replyTo,
      },
      () => {},
    );

    expect(mocks.editQueuedRun).toHaveBeenCalledOnce();
    const sentRecords = mocks.editQueuedRun.mock.calls[0]?.[0] as {
      input: {
        edit: {
          context: {
            records: ReadonlyArray<{
              kind: string;
              contextId: string;
              screenshotContextId?: string;
            }>;
          };
        };
      };
    };
    const recordIds = sentRecords.input.edit.context.records.map((record) => record.contextId);
    expect(recordIds).toEqual(
      expect.arrayContaining([
        previewRecord.contextId,
        shotImageRecord.contextId,
        thread.contextId,
        machine.contextId,
        inlineImageRecord.contextId,
      ]),
    );
    expect(recordIds).not.toContain(terminalRecord.contextId);
    expect(recordIds).not.toContain(reviewRecord.contextId);
    expect(recordIds).not.toContain(inlineFileRecord.contextId);
    expect(recordIds).not.toContain(reply.contextId);
    expect(
      sentRecords.input.edit.context.records.find(
        (record) => record.contextId === previewRecord.contextId,
      )?.screenshotContextId,
    ).toBe(shotImageRecord.contextId);
  });

  it("removes inline file and image tokens from draft.prompt in removeEditingAttachment", async () => {
    const { hook } = setup();
    const messageId = MessageId.make("message-1");
    const fileAttachment = {
      type: "file" as const,
      id: ChatAttachmentId.make("att-file-1"),
      name: "notes.txt",
      mimeType: "text/plain",
      sizeBytes: 20,
    };
    const imageAttachment = {
      type: "image" as const,
      id: ChatAttachmentId.make("att-img-1"),
      name: "shot.png",
      mimeType: "image/png",
      sizeBytes: 15,
    };
    const fileRecord = {
      version: 1 as const,
      kind: "file" as const,
      contextId: toKindScopedComposerContextId("file", "inline-file"),
      label: fileAttachment.name,
      attachmentId: fileAttachment.id,
      name: fileAttachment.name,
      mimeType: fileAttachment.mimeType,
      sizeBytes: fileAttachment.sizeBytes,
    };
    const imageRecord = {
      version: 1 as const,
      kind: "image" as const,
      contextId: toKindScopedComposerContextId("image", "inline-img"),
      label: imageAttachment.name,
      attachmentId: imageAttachment.id,
      name: imageAttachment.name,
      mimeType: imageAttachment.mimeType,
      sizeBytes: imageAttachment.sizeBytes,
    };
    const text = [
      "Inspect",
      formatInlineContextReference({
        kind: "file",
        contextId: fileRecord.contextId,
        label: fileRecord.label,
      }),
      "and",
      formatInlineContextReference({
        kind: "image",
        contextId: imageRecord.contextId,
        label: imageRecord.label,
      }),
      "carefully",
    ].join(" ");
    const messages = [
      { id: messageId, text, context: { version: 1, records: [fileRecord, imageRecord] } },
    ];
    await using _screen = await render(view(THREAD_A, "queued", hook, messages));
    await act(() =>
      requireHook(hook).beginEdit({
        runId: RUN,
        messageId,
        text,
        attachments: [fileAttachment, imageAttachment],
      }),
    );

    await act(() => requireHook(hook).removeEditingAttachment(fileAttachment.id));
    expect(
      useComposerDraftStore.getState().getComposerDraft(requireHook(hook).draftTarget)?.prompt,
    ).not.toContain(fileRecord.contextId);
    expect(
      useComposerDraftStore.getState().getComposerDraft(requireHook(hook).draftTarget)?.prompt,
    ).toContain(imageRecord.contextId);

    await act(() => requireHook(hook).removeEditingAttachment(imageAttachment.id));
    expect(
      useComposerDraftStore.getState().getComposerDraft(requireHook(hook).draftTarget)?.prompt,
    ).toBe("Inspect and carefully");
  });

  it("keeps hydrated inline context records and prunes deleted ones on kept recovery", async () => {
    const { hook } = setup();
    const messageId = MessageId.make("message-1");
    const terminalDraft = {
      id: "term-ctx-1",
      threadId: THREAD_A.threadId,
      createdAt: "2026-01-01T00:00:00.000Z",
      terminalId: "default",
      terminalLabel: "Terminal 1",
      lineStart: 1,
      lineEnd: 2,
      text: "error line",
    };
    const reviewComment: ReviewCommentContext = {
      id: "review-ctx-1",
      sectionId: "diff:src/app.ts",
      sectionTitle: "src/app.ts",
      filePath: "src/app.ts",
      startIndex: 0,
      endIndex: 1,
      rangeLabel: "+1 to +2",
      text: "Fix typo",
      diff: "+const a = 1;",
    };
    const fileAttachment = {
      type: "file" as const,
      id: ChatAttachmentId.make("att-file-1"),
      name: "notes.txt",
      mimeType: "text/plain",
      sizeBytes: 20,
    };
    const fileRecord = {
      version: 1 as const,
      kind: "file" as const,
      contextId: toKindScopedComposerContextId("file", "inline-file"),
      label: fileAttachment.name,
      attachmentId: fileAttachment.id,
      name: fileAttachment.name,
      mimeType: fileAttachment.mimeType,
      sizeBytes: fileAttachment.sizeBytes,
    };
    const terminalRecord = terminalContextRecord(terminalDraft);
    const reviewRecord = reviewCommentContextRecord(reviewComment);
    const text = [
      formatInlineContextReference(terminalContextReference(terminalDraft)),
      formatInlineContextReference(reviewCommentContextReference(reviewComment)),
      formatInlineContextReference({
        kind: "file",
        contextId: fileRecord.contextId,
        label: fileRecord.label,
      }),
    ].join(" ");
    const messages = [
      {
        id: messageId,
        text,
        context: { version: 1, records: [terminalRecord, reviewRecord, fileRecord] },
      },
    ];
    await using _screen = await render(view(THREAD_A, "queued", hook, messages));
    await act(() =>
      requireHook(hook).beginEdit({
        runId: RUN,
        messageId,
        text,
        attachments: [fileAttachment],
      }),
    );

    const target = requireHook(hook).draftTarget;
    const withoutReview = removeInlineContextReference(
      useComposerDraftStore.getState().getComposerDraft(target)!.prompt,
      reviewRecord.contextId,
    ).prompt;
    await act(() =>
      useComposerDraftStore.getState().setPrompt(target, `${withoutReview} follow-up`),
    );

    await act(() => setRuns(THREAD_A, "running"));

    const recovered = useComposerDraftStore.getState().getComposerDraft(THREAD_A);
    expect(recovered?.prompt).toBe(`${withoutReview} follow-up`);
    expect(recovered?.terminalContexts).toHaveLength(1);
    expect(recovered?.reviewComments).toEqual([]);
    expect(recovered?.files).toEqual([
      expect.objectContaining({
        id: "inline-file",
        name: fileAttachment.name,
        uploadedAttachmentId: fileAttachment.id,
        uploadEnvironmentId: ENV,
      }),
    ]);
  });
});

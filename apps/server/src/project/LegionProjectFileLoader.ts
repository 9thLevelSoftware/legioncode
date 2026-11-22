import * as Context from "effect/Context";
import * as Effect from "effect/Effect";
import * as FileSystem from "effect/FileSystem";
import * as Layer from "effect/Layer";
import * as Option from "effect/Option";
import * as Path from "effect/Path";
import * as Schema from "effect/Schema";

import { LEGION_PROJECT_FILE_NAME, type LegionProjectFile } from "@legioncode/contracts";
import { LegionProjectFileFromJson } from "@legioncode/shared/legionProjectFile";

const decodeLegionProjectFileJson = Schema.decodeEffect(LegionProjectFileFromJson);

export class LegionProjectFileLoadError extends Schema.TaggedError<LegionProjectFileLoadError>()(
  "LegionProjectFileLoadError",
  {
    operation: Schema.Literals(["read", "decode"]),
    workspaceRoot: Schema.String,
    filePath: Schema.String,
    cause: Schema.Defect(),
  },
) {
  override get message(): string {
    return `Failed to ${this.operation} ${LEGION_PROJECT_FILE_NAME} at ${this.filePath}.`;
  }
}

export class LegionProjectFileLoader extends Context.Service<
  LegionProjectFileLoader,
  {
    /** Never fails; missing/unreadable/invalid files return none, with warnings for invalid files. */
    readonly load: (workspaceRoot: string) => Effect.Effect<Option.Option<LegionProjectFile>>;
  }
>()("legioncode/project/LegionProjectFileLoader") {}

const logLegionProjectFileLoadError = (error: LegionProjectFileLoadError) =>
  Effect.logWarning(error).pipe(
    Effect.annotateLogs({
      operation: error.operation,
      workspaceRoot: error.workspaceRoot,
      filePath: error.filePath,
      errorTag: error._tag,
    }),
  );

/** @public */
export const make = Effect.gen(function* () {
  const fileSystem = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;

  const load: LegionProjectFileLoader["Service"]["load"] = Effect.fn(
    "LegionProjectFileLoader.load",
  )(function* (workspaceRoot) {
    const filePath = path.join(workspaceRoot, LEGION_PROJECT_FILE_NAME);
    const raw = yield* fileSystem.readFileString(filePath).pipe(
      Effect.asSome,
      Effect.catchTags({
        PlatformError: (error) =>
          error.reason._tag === "NotFound"
            ? Effect.succeed(Option.none<string>())
            : logLegionProjectFileLoadError(
                new LegionProjectFileLoadError({
                  operation: "read",
                  workspaceRoot,
                  filePath,
                  cause: error,
                }),
              ).pipe(Effect.as(Option.none<string>())),
      }),
    );
    if (Option.isNone(raw)) {
      return Option.none<LegionProjectFile>();
    }
    return yield* decodeLegionProjectFileJson(raw.value).pipe(
      Effect.asSome,
      Effect.catchTags({
        SchemaError: (error) =>
          logLegionProjectFileLoadError(
            new LegionProjectFileLoadError({
              operation: "decode",
              workspaceRoot,
              filePath,
              cause: error,
            }),
          ).pipe(Effect.as(Option.none<LegionProjectFile>())),
      }),
    );
  });

  return LegionProjectFileLoader.of({ load });
});

export const layer = Layer.effect(LegionProjectFileLoader, make);

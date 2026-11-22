#!/usr/bin/env node

import * as NodeRuntime from "@effect/platform-node/NodeRuntime";
import * as NodeServices from "@effect/platform-node/NodeServices";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Console from "effect/Console";
import * as Effect from "effect/Effect";
import * as FileSystem from "effect/FileSystem";
import * as Option from "effect/Option";
import * as Path from "effect/Path";
import * as Schema from "effect/Schema";
import * as Stream from "effect/Stream";
import { Command, Flag } from "effect/cli";
import { ChildProcess, ChildProcessSpawner } from "effect/process";
import sharp from "sharp";

import { BRAND_ASSET_PATHS, DEVELOPMENT_PUBLIC_ICON_OVERRIDES } from "./lib/brand-assets.ts";
import { encodePngIco, readPngDimensions, WINDOWS_ICON_SIZES } from "./lib/icon-export.ts";

const DESIGN_GENERATION = 26;
const COMPOSER_CANVAS_PT = 1024;
const MACOS_BODY_SIZE = 824;
const MACOS_BODY_INSET = 100;
const IOS_CORNER_RADIUS = 230;
const MACOS_CORNER_RADIUS = 185;
const SHARP_SVG_DENSITY = 300;
const ICON_COMPOSER_EXECUTABLE_PARTS = [
  "Contents",
  "Applications",
  "Icon Composer.app",
  "Contents",
  "Executables",
  "ictool",
] as const;
const STANDALONE_ICON_COMPOSER_EXECUTABLE_PARTS = [
  "Icon Composer.app",
  "Contents",
  "Executables",
  "ictool",
] as const;
const NonNegativeInt = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0));
const RENDERERS = ["auto", "ictool", "sharp"] as const;
type IconRenderer = (typeof RENDERERS)[number];

const IconComposerVersion = Schema.Struct({
  "bundle-version": Schema.NonEmptyString,
  "short-bundle-version": Schema.NonEmptyString,
});
const decodeIconComposerVersion = Schema.decodeUnknownEffect(
  Schema.fromJsonString(IconComposerVersion),
);

const IconJsonLayer = Schema.Struct({
  hidden: Schema.optional(Schema.Boolean),
  "image-name": Schema.String,
  name: Schema.optional(Schema.String),
  opacity: Schema.optional(Schema.Number),
  position: Schema.optional(
    Schema.Struct({
      scale: Schema.optional(Schema.Number),
      "translation-in-points": Schema.optional(Schema.Tuple([Schema.Number, Schema.Number])),
    }),
  ),
});

const IconJson = Schema.Struct({
  fill: Schema.optional(
    Schema.Struct({
      solid: Schema.optional(Schema.String),
      "automatic-gradient": Schema.optional(Schema.String),
    }),
  ),
  groups: Schema.Array(
    Schema.Struct({
      layers: Schema.Array(IconJsonLayer),
    }),
  ),
});
const decodeIconJson = Schema.decodeUnknownEffect(Schema.fromJsonString(IconJson));

type IconPlatform = "iOS";

interface VariantOutputs {
  readonly ios: string;
  readonly macos: string;
  readonly universal: string;
  readonly appleTouch: string;
  readonly webIcon192: string;
  readonly webIcon512: string;
  readonly favicon16: string;
  readonly favicon32: string;
  readonly faviconIco: string;
  readonly windowsIco: string;
}

interface IconVariant {
  readonly label: string;
  readonly source: string;
  readonly outputs: VariantOutputs;
}

interface IconComposerTool {
  readonly path: string;
  readonly version: string;
  readonly bundleVersion: string;
  readonly supportsDesignGeneration: boolean;
}

interface CommandResult {
  readonly stdout: string;
  readonly stderr: string;
  readonly exitCode: number;
}

export class IconExportFileSystemError extends Schema.TaggedError<IconExportFileSystemError>()(
  "IconExportFileSystemError",
  {
    operation: Schema.Literals([
      "resolve-repository-root",
      "check-path",
      "read-directory",
      "read-file",
      "make-directory",
      "make-temp-directory",
      "make-temp-file",
      "write-file",
      "rename-file",
    ]),
    path: Schema.String,
    cause: Schema.Defect(),
  },
) {
  override get message(): string {
    return `Icon export file-system operation '${this.operation}' failed for ${this.path}.`;
  }
}

export class IconExportProcessError extends Schema.TaggedError<IconExportProcessError>()(
  "IconExportProcessError",
  {
    operation: Schema.Literals(["spawn", "collect-stdout", "collect-stderr", "wait-for-exit"]),
    command: Schema.String,
    argumentCount: NonNegativeInt,
    cause: Schema.Defect(),
  },
) {
  override get message(): string {
    return `Icon export process operation '${this.operation}' failed for ${this.command}.`;
  }
}

export class IconExportCommandFailedError extends Schema.TaggedError<IconExportCommandFailedError>()(
  "IconExportCommandFailedError",
  {
    command: Schema.String,
    argumentCount: NonNegativeInt,
    exitCode: Schema.Int,
    sourcePath: Schema.String,
    size: Schema.Int,
    stdout: Schema.optional(Schema.String),
    stderr: Schema.optional(Schema.String),
  },
) {
  override get message(): string {
    return `Icon Composer failed to export ${this.sourcePath} at ${this.size}x${this.size}.`;
  }
}

export class IconExportToolResolutionError extends Schema.TaggedError<IconExportToolResolutionError>()(
  "IconExportToolResolutionError",
  {
    reason: Schema.Literals(["configured-invalid", "configured-outdated", "not-found"]),
    designGeneration: Schema.Int,
    toolPath: Schema.optional(Schema.String),
    version: Schema.optional(Schema.String),
  },
) {
  override get message(): string {
    switch (this.reason) {
      case "configured-invalid":
        return `ICON_COMPOSER_TOOL does not point to Icon Composer's export-capable ictool: ${this.toolPath}`;
      case "configured-outdated":
        return `ICON_COMPOSER_TOOL points to Icon Composer ${this.version}, but version 2 or newer is required for design generation ${this.designGeneration}.`;
      case "not-found":
        return `Could not find an Icon Composer 2.x exporter compatible with design generation ${this.designGeneration}. Install a compatible Icon Composer/Xcode or set ICON_COMPOSER_TOOL to Icon Composer.app/Contents/Executables/ictool.`;
    }
  }
}

export class IconExportSourceMissingError extends Schema.TaggedError<IconExportSourceMissingError>()(
  "IconExportSourceMissingError",
  {
    sourcePath: Schema.String,
  },
) {
  override get message(): string {
    return `Missing Icon Composer source project: ${this.sourcePath}`;
  }
}

export class IconExportRenditionError extends Schema.TaggedError<IconExportRenditionError>()(
  "IconExportRenditionError",
  {
    sourcePath: Schema.String,
    outputPath: Schema.String,
    expectedSize: Schema.Int,
    actualWidth: Schema.optional(Schema.Int),
    actualHeight: Schema.optional(Schema.Int),
    cause: Schema.optional(Schema.Defect()),
  },
) {
  override get message(): string {
    const actual =
      this.actualWidth === undefined || this.actualHeight === undefined
        ? "an invalid PNG"
        : `${this.actualWidth}x${this.actualHeight}`;
    return `Icon Composer produced ${actual}; expected ${this.expectedSize}x${this.expectedSize} for ${this.sourcePath}.`;
  }
}

export class IconExportSharpRenderError extends Schema.TaggedError<IconExportSharpRenderError>()(
  "IconExportSharpRenderError",
  {
    variant: Schema.String,
    stage: Schema.String,
    cause: Schema.Defect(),
  },
) {
  override get message(): string {
    return `Sharp compositor failed for ${this.variant} during ${this.stage}.`;
  }
}

export class IconExportEncodingError extends Schema.TaggedError<IconExportEncodingError>()(
  "IconExportEncodingError",
  {
    variant: Schema.String,
    cause: Schema.Defect(),
  },
) {
  override get message(): string {
    return `Failed to encode ICO renditions for the ${this.variant} icon.`;
  }
}

export class IconExportAssetsStaleError extends Schema.TaggedError<IconExportAssetsStaleError>()(
  "IconExportAssetsStaleError",
  {
    paths: Schema.Array(Schema.String),
  },
) {
  override get message(): string {
    return `Generated icon assets are stale:\n${this.paths.map((path) => `- ${path}`).join("\n")}`;
  }
}

const ICON_VARIANTS = [
  {
    label: "development",
    source: BRAND_ASSET_PATHS.developmentIconComposerProject,
    outputs: {
      ios: BRAND_ASSET_PATHS.developmentIosIconPng,
      macos: BRAND_ASSET_PATHS.developmentDesktopIconPng,
      universal: BRAND_ASSET_PATHS.developmentUniversalIconPng,
      appleTouch: BRAND_ASSET_PATHS.developmentWebAppleTouchIconPng,
      webIcon192: BRAND_ASSET_PATHS.developmentWebIcon192Png,
      webIcon512: BRAND_ASSET_PATHS.developmentWebIcon512Png,
      favicon16: BRAND_ASSET_PATHS.developmentWebFavicon16Png,
      favicon32: BRAND_ASSET_PATHS.developmentWebFavicon32Png,
      faviconIco: BRAND_ASSET_PATHS.developmentWebFaviconIco,
      windowsIco: BRAND_ASSET_PATHS.developmentWindowsIconIco,
    },
  },
  {
    label: "preview",
    source: BRAND_ASSET_PATHS.nightlyIconComposerProject,
    outputs: {
      ios: BRAND_ASSET_PATHS.nightlyIosIconPng,
      macos: BRAND_ASSET_PATHS.nightlyMacIconPng,
      universal: BRAND_ASSET_PATHS.nightlyLinuxIconPng,
      appleTouch: BRAND_ASSET_PATHS.nightlyWebAppleTouchIconPng,
      webIcon192: BRAND_ASSET_PATHS.nightlyWebIcon192Png,
      webIcon512: BRAND_ASSET_PATHS.nightlyWebIcon512Png,
      favicon16: BRAND_ASSET_PATHS.nightlyWebFavicon16Png,
      favicon32: BRAND_ASSET_PATHS.nightlyWebFavicon32Png,
      faviconIco: BRAND_ASSET_PATHS.nightlyWebFaviconIco,
      windowsIco: BRAND_ASSET_PATHS.nightlyWindowsIconIco,
    },
  },
  {
    label: "production",
    source: BRAND_ASSET_PATHS.productionIconComposerProject,
    outputs: {
      ios: BRAND_ASSET_PATHS.productionIosIconPng,
      macos: BRAND_ASSET_PATHS.productionMacIconPng,
      universal: BRAND_ASSET_PATHS.productionLinuxIconPng,
      appleTouch: BRAND_ASSET_PATHS.productionWebAppleTouchIconPng,
      webIcon192: BRAND_ASSET_PATHS.productionWebIcon192Png,
      webIcon512: BRAND_ASSET_PATHS.productionWebIcon512Png,
      favicon16: BRAND_ASSET_PATHS.productionWebFavicon16Png,
      favicon32: BRAND_ASSET_PATHS.productionWebFavicon32Png,
      faviconIco: BRAND_ASSET_PATHS.productionWebFaviconIco,
      windowsIco: BRAND_ASSET_PATHS.productionWindowsIconIco,
    },
  },
] as const satisfies ReadonlyArray<IconVariant>;

const RepositoryRoot = Effect.service(Path.Path).pipe(
  Effect.flatMap((path) => path.fromFileUrl(new URL("..", import.meta.url))),
  Effect.mapError(
    (cause) =>
      new IconExportFileSystemError({
        operation: "resolve-repository-root",
        path: new URL("..", import.meta.url).pathname,
        cause,
      }),
  ),
);

const collectStreamAsString = <E>(stream: Stream.Stream<Uint8Array, E>) =>
  stream.pipe(
    Stream.decodeText(),
    Stream.runFold(
      () => "",
      (acc, chunk) => acc + chunk,
    ),
  );

const runCommand = Effect.fn("iconExport.runCommand")(function* (
  command: string,
  args: ReadonlyArray<string>,
) {
  const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
  const child = yield* spawner.spawn(ChildProcess.make(command, args)).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportProcessError({
          operation: "spawn",
          command,
          argumentCount: args.length,
          cause,
        }),
    ),
  );
  const [stdout, stderr, exitCode] = yield* Effect.all(
    [
      collectStreamAsString(child.stdout).pipe(
        Effect.mapError(
          (cause) =>
            new IconExportProcessError({
              operation: "collect-stdout",
              command,
              argumentCount: args.length,
              cause,
            }),
        ),
      ),
      collectStreamAsString(child.stderr).pipe(
        Effect.mapError(
          (cause) =>
            new IconExportProcessError({
              operation: "collect-stderr",
              command,
              argumentCount: args.length,
              cause,
            }),
        ),
      ),
      child.exitCode.pipe(
        Effect.map(Number),
        Effect.mapError(
          (cause) =>
            new IconExportProcessError({
              operation: "wait-for-exit",
              command,
              argumentCount: args.length,
              cause,
            }),
        ),
      ),
    ],
    { concurrency: "unbounded" },
  );

  return { stdout, stderr, exitCode } satisfies CommandResult;
});

const iconComposerToolFromDeveloperDirectory = (developerDirectory: string, path: Path.Path) =>
  path.resolve(developerDirectory, "..", ...ICON_COMPOSER_EXECUTABLE_PARTS.slice(1));

const readSelectedDeveloperDirectory = Effect.fn("iconExport.readSelectedDeveloperDirectory")(
  function* () {
    const result = yield* runCommand("xcode-select", ["-p"]).pipe(Effect.option);
    return Option.flatMap(result, (output) => {
      const developerDirectory = output.stdout.trim();
      return output.exitCode === 0 && developerDirectory.length > 0
        ? Option.some(developerDirectory)
        : Option.none();
    });
  },
);

const findXcodeAppCandidates = Effect.fn("iconExport.findXcodeAppCandidates")(function* (
  directory: string,
) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const entries = yield* fs.readDirectory(directory).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "read-directory",
          path: directory,
          cause,
        }),
    ),
    Effect.orElseSucceed(() => []),
  );
  return entries
    .filter((entry) => /^Xcode.*\.app$/.test(entry))
    .map((entry) => path.join(directory, entry, ...ICON_COMPOSER_EXECUTABLE_PARTS));
});

const probeIconComposerTool = Effect.fn("iconExport.probeIconComposerTool")(function* (
  candidate: string,
) {
  const fs = yield* FileSystem.FileSystem;
  const exists = yield* fs.exists(candidate).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "check-path",
          path: candidate,
          cause,
        }),
    ),
    Effect.orElseSucceed(() => false),
  );
  if (!exists) return Option.none<IconComposerTool>();

  const result = yield* runCommand(candidate, ["--version"]).pipe(Effect.option);
  if (Option.isNone(result) || result.value.exitCode !== 0) {
    return Option.none<IconComposerTool>();
  }

  const version = yield* decodeIconComposerVersion(result.value.stdout).pipe(Effect.option);
  if (Option.isNone(version)) return Option.none<IconComposerTool>();

  const bundleVersion = version.value["bundle-version"];
  const shortVersion = version.value["short-bundle-version"];
  return Option.some({
    path: candidate,
    version: `${shortVersion} (${bundleVersion})`,
    bundleVersion,
    supportsDesignGeneration: Number.parseInt(shortVersion, 10) >= 2,
  });
});

const resolveIconComposerTool = Effect.fn("iconExport.resolveIconComposerTool")(function* () {
  const path = yield* Path.Path;
  const environment = yield* HostProcess.Environment;
  const configuredTool = environment.ICON_COMPOSER_TOOL?.trim();
  if (configuredTool) {
    const tool = yield* probeIconComposerTool(configuredTool);
    if (Option.isNone(tool)) {
      return yield* new IconExportToolResolutionError({
        reason: "configured-invalid",
        designGeneration: DESIGN_GENERATION,
        toolPath: configuredTool,
      });
    }
    if (!tool.value.supportsDesignGeneration) {
      return yield* new IconExportToolResolutionError({
        reason: "configured-outdated",
        designGeneration: DESIGN_GENERATION,
        toolPath: configuredTool,
        version: tool.value.version,
      });
    }
    return tool.value;
  }

  const selectedDeveloperDirectory = yield* readSelectedDeveloperDirectory();
  const configuredDeveloperDirectory = environment.DEVELOPER_DIR?.trim();
  const homeDirectory = environment.HOME?.trim();
  const searchDirectories = [
    "/Applications",
    ...(homeDirectory ? [path.join(homeDirectory, "Downloads")] : []),
  ];
  const xcodeCandidates = yield* Effect.forEach(searchDirectories, findXcodeAppCandidates, {
    concurrency: "unbounded",
  });
  const candidates = new Set<string>([
    ...(configuredDeveloperDirectory
      ? [iconComposerToolFromDeveloperDirectory(configuredDeveloperDirectory, path)]
      : []),
    ...Option.match(selectedDeveloperDirectory, {
      onNone: () => [],
      onSome: (developerDirectory) => [
        iconComposerToolFromDeveloperDirectory(developerDirectory, path),
      ],
    }),
    path.join("/Applications", ...STANDALONE_ICON_COMPOSER_EXECUTABLE_PARTS),
    ...(homeDirectory
      ? [path.join(homeDirectory, "Applications", ...STANDALONE_ICON_COMPOSER_EXECUTABLE_PARTS)]
      : []),
    ...xcodeCandidates.flat(),
  ]);
  const probed = yield* Effect.forEach([...candidates], probeIconComposerTool, {
    concurrency: "unbounded",
  });
  const compatibleTools = probed
    .filter(Option.isSome)
    .map((tool) => tool.value)
    .filter((tool) => tool.supportsDesignGeneration)
    .sort((left, right) =>
      right.bundleVersion.localeCompare(left.bundleVersion, undefined, { numeric: true }),
    );
  const newestTool = compatibleTools[0];
  if (newestTool) return newestTool;

  return yield* new IconExportToolResolutionError({
    reason: "not-found",
    designGeneration: DESIGN_GENERATION,
  });
});

const parseComposerColor = (
  fill:
    | { readonly solid?: string | undefined; readonly "automatic-gradient"?: string | undefined }
    | undefined,
): { readonly r: number; readonly g: number; readonly b: number; readonly alpha: number } => {
  const raw = fill?.solid ?? fill?.["automatic-gradient"];
  const match = raw
    ? /^(?:display-p3|srgb):([^,]+),([^,]+),([^,]+),([^,]+)$/u.exec(raw.trim())
    : null;
  if (!match) {
    return { r: 0, g: 0, b: 0, alpha: 1 };
  }
  const clampByte = (value: string | undefined) =>
    Math.max(0, Math.min(255, Math.round(Number.parseFloat(value ?? "0") * 255)));
  const clampAlpha = (value: string | undefined) =>
    Math.max(0, Math.min(1, Number.parseFloat(value ?? "1")));
  return {
    r: clampByte(match[1]),
    g: clampByte(match[2]),
    b: clampByte(match[3]),
    alpha: clampAlpha(match[4]),
  };
};

const parseSvgNaturalDimensions = (
  svg: string,
): { readonly width: number; readonly height: number } => {
  const widthMatch = /\bwidth="([\d.]+)"/u.exec(svg);
  const heightMatch = /\bheight="([\d.]+)"/u.exec(svg);
  if (widthMatch?.[1] && heightMatch?.[1]) {
    return {
      width: Number.parseFloat(widthMatch[1]),
      height: Number.parseFloat(heightMatch[1]),
    };
  }
  const viewBoxMatch = /\bviewBox="[^"]*?\s+[^"]*?\s+([\d.]+)\s+([\d.]+)"/u.exec(svg);
  if (viewBoxMatch?.[1] && viewBoxMatch?.[2]) {
    return {
      width: Number.parseFloat(viewBoxMatch[1]),
      height: Number.parseFloat(viewBoxMatch[2]),
    };
  }
  return { width: 128, height: 128 };
};

const normalizeLayerSvg = (
  imageName: string,
  source: string,
  opacity: number | undefined,
): string => {
  // Remove per-layer rounded-corner clips on backgrounds and librsvg-smeared cloud blur so the icon mask stays crisp.
  let normalized = source
    .replace(/<rect width="128" height="128" rx="10"\/>/u, '<rect width="128" height="128"/>')
    .replace(/ filter="url\(#soft\)"/gu, "");
  if (opacity !== undefined && opacity < 1) {
    normalized = normalized.replace(/(<svg\b[^>]*>)/u, `$1<g opacity="${opacity.toFixed(4)}">`);
    normalized = normalized.replace(/<\/svg>\s*$/u, "</g></svg>");
  }
  void imageName;
  return normalized;
};

const sharpRenderError = (variant: string, stage: string) => (cause: unknown) =>
  new IconExportSharpRenderError({ variant, stage, cause });

const renderCanvasWithSharp = Effect.fn("iconExport.renderCanvasWithSharp")(function* (
  variantLabel: string,
  assetDir: string,
  iconJson: typeof IconJson.Type,
  canvasSize: number,
  cornerRadius: number,
) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const k = canvasSize / COMPOSER_CANVAS_PT;
  const backgroundColor = parseComposerColor(iconJson.fill);
  const backToFrontLayers = [...iconJson.groups]
    .reverse()
    .flatMap((group) => [...group.layers].reverse())
    .filter((layer) => layer.hidden !== true);

  const overlays: Array<{ readonly input: Buffer; readonly left: number; readonly top: number }> =
    [];

  for (const layer of backToFrontLayers) {
    const svgPath = path.join(assetDir, layer["image-name"]);
    const rawSvg = yield* fs.readFileString(svgPath).pipe(
      Effect.mapError(
        (cause) =>
          new IconExportFileSystemError({
            operation: "read-file",
            path: svgPath,
            cause,
          }),
      ),
    );
    const natural = parseSvgNaturalDimensions(rawSvg);
    const scale = layer.position?.scale ?? 1;
    const [tx, ty] = layer.position?.["translation-in-points"] ?? [0, 0];
    const layerWidth = Math.max(1, Math.round(natural.width * scale * k));
    const layerHeight = Math.max(1, Math.round(natural.height * scale * k));
    const left = Math.round((COMPOSER_CANVAS_PT / 2 + tx) * k - layerWidth / 2);
    const top = Math.round((COMPOSER_CANVAS_PT / 2 + ty) * k - layerHeight / 2);

    const x0 = Math.max(0, left);
    const y0 = Math.max(0, top);
    const x1 = Math.min(canvasSize, left + layerWidth);
    const y1 = Math.min(canvasSize, top + layerHeight);
    if (x1 <= x0 || y1 <= y0) continue;

    const svgBuffer = Buffer.from(normalizeLayerSvg(layer["image-name"], rawSvg, layer.opacity));
    const rasterized = yield* Effect.tryPromise({
      try: () =>
        sharp(svgBuffer, { density: SHARP_SVG_DENSITY })
          .resize(layerWidth, layerHeight)
          .png({ compressionLevel: 9 })
          .toBuffer(),
      catch: sharpRenderError(variantLabel, `rasterize-${layer["image-name"]}-${canvasSize}`),
    });
    const clipped =
      x0 === left && y0 === top && x1 - x0 === layerWidth && y1 - y0 === layerHeight
        ? rasterized
        : yield* Effect.tryPromise({
            try: () =>
              sharp(rasterized)
                .extract({
                  left: x0 - left,
                  top: y0 - top,
                  width: x1 - x0,
                  height: y1 - y0,
                })
                .png({ compressionLevel: 9 })
                .toBuffer(),
            catch: sharpRenderError(variantLabel, `crop-${layer["image-name"]}-${canvasSize}`),
          });
    overlays.push({ input: clipped, left: x0, top: y0 });
  }

  const composed = yield* Effect.tryPromise({
    try: () =>
      sharp({
        create: {
          width: canvasSize,
          height: canvasSize,
          channels: 4,
          background: backgroundColor,
        },
      })
        .composite(overlays)
        .png({ compressionLevel: 9 })
        .toBuffer(),
    catch: sharpRenderError(variantLabel, `composite-${canvasSize}`),
  });

  const maskSvg = Buffer.from(
    `<svg width="${canvasSize}" height="${canvasSize}" viewBox="0 0 ${canvasSize} ${canvasSize}" xmlns="http://www.w3.org/2000/svg"><rect width="${canvasSize}" height="${canvasSize}" rx="${cornerRadius}" fill="white"/></svg>`,
  );
  return yield* Effect.tryPromise({
    try: () =>
      sharp(composed)
        .composite([{ input: maskSvg, blend: "dest-in" }])
        .png({ compressionLevel: 9 })
        .toBuffer(),
    catch: sharpRenderError(variantLabel, `mask-${canvasSize}`),
  });
});

const renderMacosPreTahoeWithSharp = Effect.fn("iconExport.renderMacosPreTahoeWithSharp")(
  function* (variantLabel: string, assetDir: string, iconJson: typeof IconJson.Type) {
    const body = yield* renderCanvasWithSharp(
      variantLabel,
      assetDir,
      iconJson,
      MACOS_BODY_SIZE,
      MACOS_CORNER_RADIUS,
    );
    // Inset the shadow caster by 1px inside the 824x824 body so dark edges never bleed along anti-aliased corners.
    const shadowSvg = Buffer.from(
      `<svg width="${COMPOSER_CANVAS_PT}" height="${COMPOSER_CANVAS_PT}" viewBox="0 0 ${COMPOSER_CANVAS_PT} ${COMPOSER_CANVAS_PT}" xmlns="http://www.w3.org/2000/svg"><defs><filter id="macShadow" x="-20%" y="-20%" width="140%" height="140%"><feDropShadow dx="0" dy="10" stdDeviation="16" flood-color="#000000" flood-opacity="0.28"/></filter></defs><rect x="${MACOS_BODY_INSET + 1}" y="${MACOS_BODY_INSET + 1}" width="${MACOS_BODY_SIZE - 2}" height="${MACOS_BODY_SIZE - 2}" rx="${MACOS_CORNER_RADIUS - 1}" fill="#000000" filter="url(#macShadow)"/></svg>`,
    );
    return yield* Effect.tryPromise({
      try: () =>
        sharp(shadowSvg, { density: 72 })
          .resize(COMPOSER_CANVAS_PT, COMPOSER_CANVAS_PT)
          .composite([{ input: body, left: MACOS_BODY_INSET, top: MACOS_BODY_INSET }])
          .png({ compressionLevel: 9 })
          .toBuffer(),
      catch: sharpRenderError(variantLabel, "macos-pre-tahoe"),
    });
  },
);

const resizeSquarePngWithSharp = (variantLabel: string, master: Buffer, size: number) =>
  Effect.tryPromise({
    try: () => sharp(master).resize(size, size).png({ compressionLevel: 9 }).toBuffer(),
    catch: sharpRenderError(variantLabel, `resize-${size}`),
  });

const renderVariantWithSharp = Effect.fn("iconExport.renderVariantWithSharp")(function* (
  repositoryRoot: string,
  variant: IconVariant,
) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const sourcePath = path.join(repositoryRoot, variant.source);
  const iconJsonPath = path.join(sourcePath, "icon.json");
  const sourceExists = yield* fs.exists(iconJsonPath).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "check-path",
          path: iconJsonPath,
          cause,
        }),
    ),
  );
  if (!sourceExists) {
    return yield* new IconExportSourceMissingError({ sourcePath: variant.source });
  }

  const rawIconJson = yield* fs.readFileString(iconJsonPath).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "read-file",
          path: iconJsonPath,
          cause,
        }),
    ),
  );
  const iconJson = yield* decodeIconJson(rawIconJson).pipe(
    Effect.mapError(sharpRenderError(variant.label, "parse-icon-json")),
  );
  const assetDir = path.join(sourcePath, "Assets");

  const ios = yield* renderCanvasWithSharp(
    variant.label,
    assetDir,
    iconJson,
    COMPOSER_CANVAS_PT,
    IOS_CORNER_RADIUS,
  );
  const macos = yield* renderMacosPreTahoeWithSharp(variant.label, assetDir, iconJson);

  const renditionCache = new Map<number, Buffer>([[COMPOSER_CANVAS_PT, ios]]);
  const resized = Effect.fn("iconExport.renderVariantWithSharp.resized")(function* (size: number) {
    const cached = renditionCache.get(size);
    if (cached) return cached;
    const buffer = yield* resizeSquarePngWithSharp(variant.label, ios, size);
    renditionCache.set(size, buffer);
    return buffer;
  });

  const icoRenditions = yield* Effect.forEach(
    WINDOWS_ICON_SIZES,
    (size) => resized(size).pipe(Effect.map((contents) => ({ size, contents }))),
    { concurrency: 1 },
  );
  const ico = yield* Effect.try({
    try: () => encodePngIco(icoRenditions),
    catch: (cause) => new IconExportEncodingError({ variant: variant.label, cause }),
  });

  return new Map<string, Buffer>([
    [variant.outputs.ios, ios],
    [variant.outputs.macos, macos],
    [variant.outputs.universal, ios],
    [variant.outputs.appleTouch, yield* resized(180)],
    [variant.outputs.webIcon192, yield* resized(192)],
    [variant.outputs.webIcon512, yield* resized(512)],
    [variant.outputs.favicon16, yield* resized(16)],
    [variant.outputs.favicon32, yield* resized(32)],
    [variant.outputs.faviconIco, ico],
    [variant.outputs.windowsIco, ico],
  ]);
});

const renderIcon = Effect.fn("iconExport.renderIcon")(function* (
  toolPath: string,
  sourcePath: string,
  outputPath: string,
  platform: IconPlatform,
  size: number,
) {
  const fs = yield* FileSystem.FileSystem;
  const args = [
    sourcePath,
    "--export-image",
    "--output-file",
    outputPath,
    "--platform",
    platform,
    "--rendition",
    "Default",
    "--width",
    String(size),
    "--height",
    String(size),
    "--scale",
    "1",
    "--design-generation",
    String(DESIGN_GENERATION),
  ];
  const result = yield* runCommand(toolPath, args);
  if (result.exitCode !== 0) {
    return yield* new IconExportCommandFailedError({
      command: toolPath,
      argumentCount: args.length,
      exitCode: result.exitCode,
      sourcePath,
      size,
      ...(result.stdout.trim() ? { stdout: result.stdout.trim() } : {}),
      ...(result.stderr.trim() ? { stderr: result.stderr.trim() } : {}),
    });
  }

  const contents = yield* fs.readFile(outputPath).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "read-file",
          path: outputPath,
          cause,
        }),
    ),
  );
  const buffer = Buffer.from(contents);
  const dimensions = yield* Effect.try({
    try: () => readPngDimensions(buffer),
    catch: (cause) =>
      new IconExportRenditionError({
        sourcePath,
        outputPath,
        expectedSize: size,
        cause,
      }),
  });
  if (dimensions.width !== size || dimensions.height !== size) {
    return yield* new IconExportRenditionError({
      sourcePath,
      outputPath,
      expectedSize: size,
      actualWidth: dimensions.width,
      actualHeight: dimensions.height,
    });
  }
  return buffer;
});

const renderVariant = Effect.fn("iconExport.renderVariant")(function* (
  toolPath: string,
  repositoryRoot: string,
  temporaryDirectory: string,
  variant: IconVariant,
) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const sourcePath = path.join(repositoryRoot, variant.source);
  const sourceExists = yield* fs.exists(sourcePath).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "check-path",
          path: sourcePath,
          cause,
        }),
    ),
  );
  if (!sourceExists) {
    return yield* new IconExportSourceMissingError({ sourcePath: variant.source });
  }

  const renditionCache = new Map<string, Buffer>();
  const render = Effect.fn("iconExport.renderVariant.rendition")(function* (
    platform: IconPlatform,
    size: number,
  ) {
    const cacheKey = `${platform}-${size}`;
    const cached = renditionCache.get(cacheKey);
    if (cached) return cached;

    const outputPath = path.join(temporaryDirectory, `${variant.label}-${platform}-${size}.png`);
    const contents = yield* renderIcon(toolPath, sourcePath, outputPath, platform, size);
    renditionCache.set(cacheKey, contents);
    return contents;
  });

  const ios = yield* render("iOS", 1024);
  const icoRenditions = yield* Effect.forEach(
    WINDOWS_ICON_SIZES,
    (size) => render("iOS", size).pipe(Effect.map((contents) => ({ size, contents }))),
    { concurrency: 1 },
  );
  const ico = yield* Effect.try({
    try: () => encodePngIco(icoRenditions),
    catch: (cause) => new IconExportEncodingError({ variant: variant.label, cause }),
  });

  return new Map<string, Buffer>([
    [variant.outputs.ios, ios],
    [variant.outputs.universal, ios],
    [variant.outputs.appleTouch, yield* render("iOS", 180)],
    [variant.outputs.webIcon192, yield* render("iOS", 192)],
    [variant.outputs.webIcon512, yield* render("iOS", 512)],
    [variant.outputs.favicon16, yield* render("iOS", 16)],
    [variant.outputs.favicon32, yield* render("iOS", 32)],
    [variant.outputs.faviconIco, ico],
    [variant.outputs.windowsIco, ico],
  ]);
});

const writeAtomically = Effect.fn("iconExport.writeAtomically")(function* (
  repositoryRoot: string,
  relativePath: string,
  contents: Buffer,
) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const targetPath = path.join(repositoryRoot, relativePath);
  const targetDirectory = path.dirname(targetPath);
  yield* fs.makeDirectory(targetDirectory, { recursive: true }).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "make-directory",
          path: targetDirectory,
          cause,
        }),
    ),
  );
  const temporaryPath = yield* fs
    .makeTempFileScoped({
      directory: targetDirectory,
      prefix: ".legioncode-icon-export-",
      suffix: ".tmp",
    })
    .pipe(
      Effect.mapError(
        (cause) =>
          new IconExportFileSystemError({
            operation: "make-temp-file",
            path: targetDirectory,
            cause,
          }),
      ),
    );
  yield* fs.writeFile(temporaryPath, contents).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "write-file",
          path: temporaryPath,
          cause,
        }),
    ),
  );
  yield* fs.rename(temporaryPath, targetPath).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "rename-file",
          path: targetPath,
          cause,
        }),
    ),
  );
});

const isCurrent = Effect.fn("iconExport.isCurrent")(function* (
  repositoryRoot: string,
  relativePath: string,
  expected: Buffer,
) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const targetPath = path.join(repositoryRoot, relativePath);
  const exists = yield* fs.exists(targetPath).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "check-path",
          path: targetPath,
          cause,
        }),
    ),
  );
  if (!exists) return false;

  const actual = yield* fs.readFile(targetPath).pipe(
    Effect.mapError(
      (cause) =>
        new IconExportFileSystemError({
          operation: "read-file",
          path: targetPath,
          cause,
        }),
    ),
  );
  return Buffer.from(actual).equals(expected);
});

const resolveSelectedBackend = Effect.fn("iconExport.resolveSelectedBackend")(function* (
  renderer: IconRenderer,
) {
  if (renderer === "sharp") {
    return Option.none<IconComposerTool>();
  }
  if (renderer === "ictool") {
    return Option.some(yield* resolveIconComposerTool());
  }
  return yield* resolveIconComposerTool().pipe(
    Effect.asSome,
    Effect.catchTags({
      IconExportToolResolutionError: (error) =>
        error.reason === "not-found" ? Effect.succeedNone : Effect.fail(error),
    }),
  );
});

export const exportBrandIcons = Effect.fn("exportBrandIcons")(function* (
  checkOnly: boolean,
  renderer: IconRenderer = "auto",
) {
  const fs = yield* FileSystem.FileSystem;
  const repositoryRoot = yield* RepositoryRoot;
  const tool = yield* resolveSelectedBackend(renderer);

  const generated = new Map<string, Buffer>();
  if (Option.isSome(tool)) {
    const temporaryDirectory = yield* fs
      .makeTempDirectoryScoped({
        prefix: "legioncode-icon-export-",
      })
      .pipe(
        Effect.mapError(
          (cause) =>
            new IconExportFileSystemError({
              operation: "make-temp-directory",
              path: "system temporary directory",
              cause,
            }),
        ),
      );
    yield* Console.log(
      `Exporting icons with Icon Composer ${tool.value.version}, design generation ${DESIGN_GENERATION}.`,
    );
    for (const variant of ICON_VARIANTS) {
      yield* Console.log(`Rendering ${variant.label} from ${variant.source}...`);
      const variantAssets = yield* renderVariant(
        tool.value.path,
        repositoryRoot,
        temporaryDirectory,
        variant,
      );
      for (const [relativePath, contents] of variantAssets) {
        generated.set(relativePath, contents);
      }
    }
  } else {
    yield* Console.log(
      "Exporting icons with cross-platform sharp SVG compositor (Icon Composer ictool not used).",
    );
    for (const variant of ICON_VARIANTS) {
      yield* Console.log(`Rendering ${variant.label} from ${variant.source}...`);
      const variantAssets = yield* renderVariantWithSharp(repositoryRoot, variant);
      for (const [relativePath, contents] of variantAssets) {
        generated.set(relativePath, contents);
      }
    }
  }

  for (const override of DEVELOPMENT_PUBLIC_ICON_OVERRIDES) {
    const sourceContents = generated.get(override.sourceRelativePath);
    if (sourceContents === undefined) {
      return yield* Effect.die(
        new Error(`Generated development web icon is missing: ${override.sourceRelativePath}`),
      );
    }
    generated.set(override.targetRelativePath, sourceContents);
  }

  if (checkOnly) {
    const stale = yield* Effect.filter(
      [...generated.entries()],
      ([relativePath, contents]) =>
        isCurrent(repositoryRoot, relativePath, contents).pipe(Effect.map((current) => !current)),
      { concurrency: "unbounded" },
    );
    if (stale.length > 0) {
      return yield* new IconExportAssetsStaleError({
        paths: stale.map(([relativePath]) => relativePath),
      });
    }
    yield* Console.log(`All ${generated.size} generated icon assets are current.`);
    return;
  }

  yield* Effect.forEach(
    generated,
    ([relativePath, contents]) => writeAtomically(repositoryRoot, relativePath, contents),
    { concurrency: 1, discard: true },
  );
  yield* Console.log(`Updated ${generated.size} generated icon assets.`);
});

export const exportBrandIconsCommand = Command.make(
  "export-brand-icons",
  {
    check: Flag.Boolean("check").pipe(
      Flag.withDescription("Verify generated icon assets without modifying files."),
      Flag.withDefault(false),
    ),
    renderer: Flag.Literals("renderer", RENDERERS).pipe(
      Flag.withDescription("Icon renderer backend to use (auto, ictool, or sharp)."),
      Flag.withDefault("auto" as const),
    ),
  },
  ({ check, renderer }) => exportBrandIcons(check, renderer).pipe(Effect.scoped),
).pipe(
  Command.withDescription(
    "Export development, preview, and production assets from Icon Composer projects.",
  ),
);

if (import.meta.main) {
  Command.run(exportBrandIconsCommand, { version: "0.0.0" }).pipe(
    Effect.provide(NodeServices.layer),
    NodeRuntime.runMain,
  );
}

import * as Exit from "effect/Exit";
import * as Schema from "effect/Schema";

import { LegionProjectFile, LEGION_PROJECT_FILE_SCHEMA_URL } from "@legioncode/contracts";

import { fromLenientJson } from "./schemaJson.ts";

export const LegionProjectFileFromJson = fromLenientJson(LegionProjectFile);

const decodeLegionProjectFile = Schema.decodeExit(LegionProjectFileFromJson);

export function parseLegionProjectFile(contents: string): LegionProjectFile | null {
  const decoded = decodeLegionProjectFile(contents);
  return Exit.isSuccess(decoded) ? decoded.value : null;
}

export function buildLegionProjectFileJsonSchema(): Record<string, unknown> {
  const document = Schema.toJsonSchemaDocument(LegionProjectFile, { onExcessProperty: "error" });
  const jsonSchema: Record<string, unknown> = {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: LEGION_PROJECT_FILE_SCHEMA_URL,
    ...document.schema,
  };
  if (document.definitions && Object.keys(document.definitions).length > 0) {
    jsonSchema.$defs = document.definitions;
  }
  return jsonSchema;
}

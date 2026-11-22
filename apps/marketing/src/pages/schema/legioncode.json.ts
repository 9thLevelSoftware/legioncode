import type { APIRoute } from "astro";

import { buildLegionProjectFileJsonSchema } from "@legioncode/shared/legionProjectFile";

export const GET: APIRoute = () =>
  new Response(`${JSON.stringify(buildLegionProjectFileJsonSchema(), null, 2)}\n`, {
    headers: { "Content-Type": "application/json" },
  });

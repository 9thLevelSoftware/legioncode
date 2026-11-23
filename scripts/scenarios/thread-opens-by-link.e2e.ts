import { expect } from "e2e";

import { test, WEB } from "./legioncode.ts";

test(
  "A seeded thread opens by its link and shows its history",
  WEB,
  async ({ legioncode, app, screen }) => {
    const project = await legioncode.project("demo");
    const thread = await legioncode.thread({
      project,
      title: "Earlier work",
      messages: ["first", "second"],
    });
    await legioncode.open(thread);

    await expect(screen.getByText("second fixture turn complete", { exact: false })).toBeVisible();
    await app.screenshot("thread");
  },
);

import { expect } from "e2e";

import { test, WEB } from "./legioncode.ts";

test(
  "An invalid host filter keeps the Issues page open",
  WEB,
  async ({ legioncode, app, browser }) => {
    await legioncode.project("demo");
    await legioncode.open("/issues?host=true");

    await app.screenshot("issues-invalid-host");
    await expect(browser.locator("h1")).toHaveText("Issues");
  },
);

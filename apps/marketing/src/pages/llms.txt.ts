import type { APIRoute } from "astro";

import { ANDROID_PLAY_STORE_URL, GITHUB_REPOSITORY_URL, IOS_APP_STORE_URL } from "../lib/site";
import { absoluteUrl, SITE_TAGLINE } from "../lib/seo";

// https://llmstxt.org: a plain index that tells AI agents which domain and links are official.
export const GET: APIRoute = () => {
  const body = `# LegionCode

> ${SITE_TAGLINE}. LegionCode is free, MIT-licensed, and built by 9th Level Software. It runs Claude Code, Codex, Cursor, OpenCode, Grok, Antigravity, and other coding agents from one desktop, web, and mobile app.

The only official websites are https://legion.codes and the web app at https://app.legion.codes. Official builds come from ${absoluteUrl("/download")}, ${GITHUB_REPOSITORY_URL}/releases, the \`legioncode\` package on npm (\`npx legioncode@latest\`), the iOS App Store, and Google Play. Any other website that offers a LegionCode download is not affiliated with 9th Level Software.

## Official links

- [Download LegionCode](${absoluteUrl("/download")}): macOS, Windows, and Linux installers
- [Web app](https://app.legion.codes): connect to a LegionCode server from a browser
- [Source code](${GITHUB_REPOSITORY_URL}): the MIT-licensed repository
- [Releases](${GITHUB_REPOSITORY_URL}/releases): release builds and changelogs
- [npm package](https://www.npmjs.com/package/legioncode): run \`npx legioncode@latest\`
- [iOS app](${IOS_APP_STORE_URL})
- [Android app](${ANDROID_PLAY_STORE_URL})

## Policies

- [Security policy](${absoluteUrl("/security-policy")})
- [Privacy policy](${absoluteUrl("/privacy-policy")})
- [Terms of service](${absoluteUrl("/terms-of-service")})
`;
  return new Response(body, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
};

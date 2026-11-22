# FAQ

## Is LegionCode a harness?

No. A harness is what turns an LLM into an agent: it gives the model tools, a way to call them, runs them, and hands the results back. Claude Code is kind of a harness. It is an interface (the TUI, the Code tab in the desktop app) plus a harness underneath (the Anthropic Agent SDK). Codex is the same split: the app plus the Codex App Server. LegionCode uses those harnesses. It is not a harness, and it does not work without one.

Chef = LLM. Kitchen = harness. LegionCode = the restaurant that lets you bring whatever kitchen you want. Our MCP is a cookbook we hand the kitchen, but the chef won't open it unless you tell them to :)

Bring your own: Claude Code, Codex, Cursor, Grok Build, OpenCode, Google Antigravity, Kiro, Pi, Muse Code, or fx. [Provider setup](./install.md#providers)

## Why use LegionCode instead of Claude Code or the Codex app?

We were inspired by the Codex desktop app, Conductor, Claude Desktop, and Cursor Glass, but none met our bar. What you get:

- Bring your own sub. Claude, Codex, and whatever else you already have installed.
- Best in class remote. Run `legioncode up` on any box and drive it from anywhere.
- Best in class mobile app.
- Truly 100% open source. If we go the wrong direction, fork it.

Every harness feels nearly identical in here, so you can stop worrying about harnesses and focus on building :) [Install](./install.md)

## Can I use LegionCode with my Claude Code subscription?

Yes.

## How much does LegionCode cost?

Nothing. There's no way to pay us to use LegionCode. You bring your own sub. It is free and open source (MIT). You still pay your provider like you already do. Legion Connect is free too, for now. Every account can connect up to 3 machines running LegionCode. Phones and browsers that connect to them do not count. Need more? Link a paid T3 Chat account. [More machines](#can-i-connect-more-than-3-machines-with-legion-connect) We don't want to charge for this, but if the bill gets unacceptable we may have to change course. If you hit limits or have issues, you can always fork it, self host, or use Tailscale. [Install](./install.md)

## Can I connect more than 3 machines with Legion Connect?

Yes. Link your T3 Chat account and Legion Connect raises your limit: up to 10 machines with T3 Chat Pro, or 100 with Premier. Open your account menu in LegionCode, go to **T3 Chat**, and select **Link T3 Chat**. Sign in to T3 Chat and approve. The free T3 Chat plan keeps the normal limit of 3. [More machines with T3 Chat](./remote-access.md#more-machines-with-t3-chat)

## Why is my Claude 5-hour limit draining so fast?

It shouldn't anymore. LegionCode drives Claude through Anthropic's Agent SDK, and Anthropic's 5h meter used to charge Agent SDK usage about 1.7x more than using Claude Code directly. Weekly limits were never affected. Since v0.1.7, Claude in LegionCode drains your 5h limit at the same rate as Claude Code. Still seeing it? [Update LegionCode](./updating.md) on the machine that runs your agents. [Tracking issue](https://github.com/9thLevelSoftware/legioncode/issues/24649)

## How do I use LegionCode remotely?

Your agents run wherever LegionCode runs: your desktop, a server, a VPS. You connect to it from anywhere. Run `legioncode up` on that machine and you get built-in remote support. Legion Connect is the easy path. LAN pairing and Tailscale work too, and [app.legion.codes](https://app.legion.codes) is the web client. [Remote access](./remote-access.md)

## How do I use LegionCode on my phone?

Get the [iOS](https://apps.apple.com/us/app/legioncode-remote-claude-more/id6787819824) or [Android](https://play.google.com/store/apps/details?id=dev.legioncode) app, then connect it to your computer with Legion Connect or a pairing link. The phone is a remote. Your computer does the work and needs to stay on. [Mobile setup](./install.md#mobile-app) · [Remote access](./remote-access.md)

## Why does Legion Connect keep disconnecting?

Legion Connect is our built-in alternative to Tailscale for remote connections. If it keeps dropping, open **Settings > Environments**, find the machine, and select **Details**. The first part marked as failed is the cause. You can also set up Tailscale as a fallback. [Legion Connect troubleshooting](./legion-connect-troubleshooting.md)

## Will I get banned for using my subscription?

No. LegionCode works within the TOS of Claude Code, Codex, and Google Antigravity, with direct integrations for Grok Build and Cursor and official ACP bindings for the rest.

If it works in LegionCode, you can trust it is safe.

## Can I add a bunch of accounts for one provider?

LegionCode avoids built-in multi-account auto-switchers so you don't risk provider TOS bans.

A separate work account is fine. Setup for two: [Codex](./providers-codex.md#use-multiple-accounts) · [Claude](./providers-claude.md#separate-accounts-or-configurations). For a bigger pool, manage them outside LegionCode with a self-hosted proxy like [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI). After you set up a hub, LegionCode can show its pooled limits under Usage. [Connect a hub](./usage.md#connect-a-cliproxyapi-hub)

## Can I use an API key or OpenRouter instead of a subscription?

Yes. Add another provider instance in Settings and give it its own keys and endpoint. Claude has a full OpenRouter recipe. Your subscription setup stays untouched. [OpenRouter](./providers-claude.md#openrouter) · [Provider instances](./install.md#providers)

## Does LegionCode support local models?

No. LegionCode is a serious developer tool. Locally runnable models are not capable of meaningful engineering work.

## Does my code go to LegionCode servers?

No. Your code and prompts go from your machine to your provider (Anthropic, OpenAI, etc.), not to us. With Legion Connect, traffic rides a Cloudflare tunnel (`cloudflared` on your machine). Our relay sets up the link. It does not proxy your HTTP or WebSocket sessions. LegionCode does send basic product usage events (provider, model, token totals, that kind of thing). No prompts, no file contents. That is the same on Tailscale, LAN, or Legion Connect. Set `LEGIONCODE_TELEMETRY_ENABLED=false` if you want it off. [Product usage data](./telemetry.md)

## Worktrees are eating my disk. How do I clean them up?

Open **Settings → Worktrees → Cleanup** and turn on automatic worktree cleanup. You can also run it now from there. Worktrees add up fast. [Storage cleanup](./project-settings.md#storage-cleanup)

## Is there voice input?

Yes. On iPhone and Android, dictation uses on-device speech by default; you can opt in to a paired environment instead. Web and desktop require you to choose an environment first. The transcript goes into your draft for review. See [Voice input](./voice-input.md).

## Does it work on Windows, Mac, and Linux?

Yes, all three. Desktop app, CLI, the works. One catch: there is no `legioncode` binary for Intel Macs, so use the desktop app there. Windows folks can also run agents inside WSL. [Install](./install.md)

## Why is my provider not showing up?

Almost always, the CLI is not installed or not on the `PATH` of the machine that runs the server. (Not your phone. The computer.) Log in on that machine first, like `codex login` or `claude auth login`. Still missing? Point **Binary path** at it in provider settings. Version managers love to cause this one. [Provider setup](./install.md#providers)

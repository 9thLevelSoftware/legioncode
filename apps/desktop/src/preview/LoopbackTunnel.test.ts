import { SshConnectionProfile, SshConnectionTarget } from "@t3tools/client-runtime/connection";
import { ConnectionCatalogDocument } from "@t3tools/client-runtime/platform";
import * as NodeFileSystem from "@effect/platform-node/NodeFileSystem";
import * as NodeWS from "@effect/platform-node/NodeWS";
import { EnvironmentId } from "@t3tools/contracts";
import * as HostProcess from "@t3tools/shared/HostProcess";
import { expect, it, vi } from "@effect/vitest";
import * as NodeNet from "node:net";
import * as Context from "effect/Context";
import * as Effect from "effect/Effect";
import * as FileSystem from "effect/FileSystem";
import * as Layer from "effect/Layer";
import * as Option from "effect/Option";
import * as Queue from "effect/Queue";
import * as Ref from "effect/Ref";
import * as Result from "effect/Result";
import * as Schema from "effect/Schema";
import * as SecureRandom from "effect/crypto/SecureRandom";

import * as DesktopConnectionCatalogStore from "../app/DesktopConnectionCatalogStore.ts";
import * as ElectronWindow from "../electron/ElectronWindow.ts";
import * as DesktopSshEnvironment from "../ssh/DesktopSshEnvironment.ts";
import { setConnectionCatalog } from "../ipc/methods/connectionCatalog.ts";
import { disconnectSshEnvironment } from "../ipc/methods/sshEnvironment.ts";
import * as LoopbackTunnel from "./LoopbackTunnel.ts";

const addressPort = (server: NodeNet.Server | NodeWS.WebSocketServer) =>
  (server.address() as NodeNet.AddressInfo).port;

const closeServer = (server: NodeNet.Server | NodeWS.WebSocketServer) =>
  Effect.promise(
    () =>
      new Promise<void>((resolve) => {
        if (server instanceof NodeWS.WebSocketServer) {
          for (const client of server.clients) client.terminate();
        }
        server.close(() => resolve());
      }),
  );

/** A local listener that must never see a connection: the reviewer's own localhost server. */
const listenDecoy = Effect.acquireRelease(
  Effect.promise(
    () =>
      new Promise<{ server: NodeNet.Server; connections: () => number }>((resolve) => {
        let connections = 0;
        const server = NodeNet.createServer((socket) => {
          connections++;
          socket.destroy();
        });
        server.listen(0, "127.0.0.1", () => resolve({ server, connections: () => connections }));
      }),
  ),
  ({ server }) => closeServer(server),
);

/** Stands in for the environment's /api/preview/tunnel: echoes each frame behind a 0xee byte. */
const listenEndpoint = Effect.acquireRelease(
  Effect.promise(
    () =>
      new Promise<{ server: NodeWS.WebSocketServer; requests: string[] }>((resolve) => {
        const requests: string[] = [];
        const server: NodeWS.WebSocketServer = new NodeWS.WebSocketServer(
          { port: 0, host: "127.0.0.1" },
          () => resolve({ server, requests }),
        );
        server.on("connection", (socket: NodeWS.WebSocket, request: { url?: string }) => {
          requests.push(request.url ?? "");
          socket.on("message", (data: Buffer) =>
            socket.send(Buffer.concat([Uint8Array.of(0xee), data])),
          );
        });
      }),
  ),
  ({ server }) => closeServer(server),
);

const unusedPort = Effect.promise(
  () =>
    new Promise<number>((resolve) => {
      const server = NodeNet.createServer();
      server.listen(0, "127.0.0.1", () => {
        const port = addressPort(server);
        server.close(() => resolve(port));
      });
    }),
);

/** The tunnel service, with every access request answered with `tunnelUrl`. */
const makeTunnel = (
  /** A function answers each access request with its current value. */
  tunnelUrl: string | null | (() => string | null),
  host?: Layer.Layer<FileSystem.FileSystem> | Layer.Layer<never>,
) =>
  Effect.gen(function* () {
    const accessRequests = yield* Queue.unbounded<string>();
    const window = {
      isDestroyed: () => false,
      webContents: {
        send: (_channel: string, request: { readonly requestId: string }) =>
          Queue.offerUnsafe(accessRequests, request.requestId),
      },
    };
    const services = yield* Layer.build(
      LoopbackTunnel.layer.pipe(
        Layer.provide(
          Layer.mergeAll(
            Layer.mock(ElectronWindow.ElectronWindow, {
              currentMainOrFirst: Effect.succeedSome(window as unknown as Electron.BrowserWindow),
            }),
            NodeFileSystem.layer,
            SecureRandom.layerWebCrypto,
            host ?? Layer.empty,
          ),
        ),
      ),
    );
    const tunnel = Context.get(services, LoopbackTunnel.LoopbackTunnel);
    yield* Queue.take(accessRequests).pipe(
      Effect.flatMap((requestId) =>
        tunnel.resolveAccess({
          requestId,
          url: typeof tunnelUrl === "function" ? tunnelUrl() : tunnelUrl,
        }),
      ),
      Effect.forever,
      Effect.forkScoped,
    );
    return tunnel;
  });

/** Routes a stand-in session through the tunnel and returns its SOCKS listener port. */
const routeThroughTunnel = (
  tunnelUrl: string | null,
  host?: Layer.Layer<FileSystem.FileSystem> | Layer.Layer<never>,
) =>
  Effect.gen(function* () {
    const tunnel = yield* makeTunnel(tunnelUrl, host);
    let proxyRules = "";
    const session = {
      setProxy: async (config: Electron.ProxyConfig) => {
        proxyRules = config.proxyRules ?? "";
      },
      closeAllConnections: async () => undefined,
      webRequest: { onBeforeRequest: () => undefined },
    };
    yield* tunnel.route({
      environmentId: "remote",
      session: session as unknown as Electron.Session,
      tunnelled: true,
    });
    return Number(/socks5:\/\/127\.0\.0\.1:(\d+)/.exec(proxyRules)![1]);
  });

const formatProcTcpRow = (entry: {
  readonly clientPort: number;
  readonly listenerPort: number;
  readonly uid: number;
}) => {
  const hexPort = (port: number) => port.toString(16).padStart(4, "0").toUpperCase();
  return [
    "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode",
    `   0: 0100007F:${hexPort(entry.clientPort)} 0100007F:${hexPort(entry.listenerPort)} 01 00000000:00000000 00:00000000 00000000 ${entry.uid} 0 0 1 0000000000000000 0 0 0 0 -1`,
  ].join("\n");
};

/**
 * The real TCP tables with 127.0.0.1 in one column rewritten to 127.0.0.2 (or a synthetic Linux
 * table when `/proc/net/tcp` does not exist on non-Linux test hosts). The rows keep their ports and
 * owner, so only an address mismatch can refuse them.
 */
const tcpTablesLayer = (
  movedColumn: 1 | 2 | undefined,
  fallbackRow: () => {
    readonly clientPort: number;
    readonly listenerPort: number;
    readonly uid: number;
  },
) =>
  Layer.effect(
    FileSystem.FileSystem,
    Effect.gen(function* () {
      const fileSystem = yield* FileSystem.FileSystem;
      const moveAddress = (line: string) => {
        if (movedColumn === undefined) return line;
        const columns = line.trim().split(/\s+/);
        const address = columns[movedColumn];
        if (address === undefined || !/0100007F:/i.test(address)) return line;
        columns[movedColumn] = address.replace(/0100007F:/i, "0200007F:");
        return columns.join(" ");
      };
      return FileSystem.FileSystem.of({
        ...fileSystem,
        readFileString: (path, encoding) =>
          fileSystem.readFileString(path, encoding).pipe(
            Effect.orElseSucceed(() =>
              path === "/proc/net/tcp" ? formatProcTcpRow(fallbackRow()) : "",
            ),
            Effect.map((table) => table.split("\n").map(moveAddress).join("\n")),
          ),
      });
    }),
  ).pipe(Layer.provide(NodeFileSystem.layer));

/** Greets the SOCKS listener and sends a CONNECT for `host:port`; resolves with the reply. */
const socksConnect = (proxyPort: number, host: string, port: number, localPort?: number) =>
  Effect.acquireRelease(
    Effect.promise(
      () =>
        new Promise<{ socket: NodeNet.Socket; reply: Buffer }>((resolve, reject) => {
          const socket = NodeNet.connect(
            {
              host: "127.0.0.1",
              port: proxyPort,
              ...(localPort === undefined ? {} : { localAddress: "127.0.0.1", localPort }),
            },
            () => socket.write(Uint8Array.of(5, 1, 0)),
          );
          let received = Buffer.alloc(0);
          let greeted = false;
          const onData = (chunk: Buffer) => {
            received = Buffer.concat([received, chunk]);
            if (!greeted && received.length >= 2) {
              // A refused greeting is the whole answer.
              if (received[1] !== 0) return resolve({ socket, reply: received.subarray(0, 2) });
              greeted = true;
              received = received.subarray(2);
              const destination = Buffer.alloc(2);
              destination.writeUInt16BE(port);
              socket.write(
                Buffer.concat([
                  Uint8Array.of(5, 1, 0, 3, host.length),
                  Buffer.from(host, "latin1"),
                  destination,
                ]),
              );
            }
            if (greeted && received.length >= 10) {
              socket.off("data", onData);
              resolve({ socket, reply: received.subarray(0, 10) });
            }
          };
          socket.on("data", onData);
          socket.on("error", reject);
        }),
    ),
    ({ socket }) => Effect.sync(() => socket.destroy()),
  );

it.effect.each(["localhost", "localhost."])(
  "carries a loopback CONNECT for %s over the tunnel WebSocket, not to local listeners",
  (host) =>
    Effect.gen(function* () {
      const decoy = yield* listenDecoy;
      const endpoint = yield* listenEndpoint;
      const proxyPort = yield* routeThroughTunnel(
        `ws://127.0.0.1:${addressPort(endpoint.server)}/api/preview/tunnel?wsTicket=t`,
      );
      const { socket, reply } = yield* socksConnect(proxyPort, host, addressPort(decoy.server));
      expect(reply[1]).toBe(0);
      const echoed = yield* Effect.promise(
        () =>
          new Promise<number[]>((resolve) => {
            socket.once("data", (chunk) => resolve([...chunk]));
            socket.write(Uint8Array.of(1, 2, 3));
          }),
      );
      expect(echoed).toEqual([0xee, 1, 2, 3]);
      expect(endpoint.requests).toEqual([
        `/api/preview/tunnel?wsTicket=t&port=${addressPort(decoy.server)}`,
      ]);
      expect(decoy.connections()).toBe(0);
    }),
);

it.effect.each([
  { case: "a non-loopback destination", host: "192.168.1.20", tunnel: "endpoint" },
  { case: "no tunnel URL", host: "127.0.0.1", tunnel: "none" },
  { case: "an unreachable tunnel endpoint", host: "localhost", tunnel: "unreachable" },
  { case: "a missing Linux user id", host: "localhost", tunnel: "missing-uid" },
  { case: "another OS user's client", host: "localhost", tunnel: "other-user" },
  { case: "a TCP row whose client address differs", host: "localhost", tunnel: "moved-client" },
  { case: "a TCP row whose listener address differs", host: "localhost", tunnel: "moved-listener" },
] as const)("fails the SOCKS request for $case without dialling locally", ({ host, tunnel }) =>
  Effect.gen(function* () {
    const decoy = yield* listenDecoy;
    const endpoint = yield* listenEndpoint;
    const endpointPort =
      tunnel === "unreachable" ? yield* unusedPort : addressPort(endpoint.server);
    const linuxUid = process.getuid?.() ?? 1000;
    const pinnedClientPort =
      tunnel === "other-user" || tunnel === "moved-client" || tunnel === "moved-listener"
        ? yield* unusedPort
        : undefined;
    let proxyPort = 0;
    const row = () => ({
      clientPort: pinnedClientPort!,
      listenerPort: proxyPort,
      uid: linuxUid,
    });
    proxyPort = yield* routeThroughTunnel(
      tunnel === "none" ? null : `ws://127.0.0.1:${endpointPort}/api/preview/tunnel`,
      tunnel === "missing-uid"
        ? Layer.mergeAll(
            Layer.succeed(HostProcess.Platform, "linux"),
            Layer.succeed(HostProcess.UserId, undefined),
          )
        : // This process owns the client, so a different user id here makes it another user's.
          tunnel === "other-user"
          ? Layer.mergeAll(
              Layer.succeed(HostProcess.Platform, "linux"),
              Layer.succeed(HostProcess.UserId, linuxUid + 1),
              tcpTablesLayer(undefined, row),
            )
          : tunnel === "moved-client" || tunnel === "moved-listener"
            ? Layer.mergeAll(
                Layer.succeed(HostProcess.Platform, "linux"),
                Layer.succeed(HostProcess.UserId, linuxUid),
                tcpTablesLayer(tunnel === "moved-client" ? 1 : 2, row),
              )
            : undefined,
    );
    const { reply } = yield* socksConnect(
      proxyPort,
      host,
      addressPort(decoy.server),
      pinnedClientPort,
    );
    expect(reply[1]).not.toBe(0);
    expect(endpoint.requests).toEqual([]);
    expect(decoy.connections()).toBe(0);
  }),
);

it.effect("connects straight to the environment's own forwarded origin port", () =>
  Effect.gen(function* () {
    // Over SSH the origin is a local forward, and signed file previews load from it.
    const received: number[] = [];
    const origin = yield* Effect.acquireRelease(
      Effect.promise(
        () =>
          new Promise<NodeNet.Server>((resolve) => {
            const server = NodeNet.createServer((socket) =>
              socket.on("data", (chunk) => {
                received.push(...chunk);
                socket.write(chunk);
              }),
            );
            server.listen(0, "127.0.0.1", () => resolve(server));
          }),
      ),
      closeServer,
    );
    const port = addressPort(origin);
    const proxyPort = yield* routeThroughTunnel(`ws://127.0.0.1:${port}/api/preview/tunnel`);
    const { socket, reply } = yield* socksConnect(proxyPort, "127.0.0.1", port);
    expect(reply[1]).toBe(0);
    const echoed = yield* Effect.promise(
      () =>
        new Promise<number[]>((resolve) => {
          socket.once("data", (chunk) => resolve([...chunk]));
          socket.write(Uint8Array.of(7, 8));
        }),
    );
    expect(echoed).toEqual([7, 8]);
    expect(received).toEqual([7, 8]);
  }),
);

/** Connects to a local port, sends `bytes`, and resolves with the first chunk back. */
const exchange = (port: number, bytes: Uint8Array) =>
  Effect.promise(
    () =>
      new Promise<number[]>((resolve, reject) => {
        const socket = NodeNet.connect(port, "127.0.0.1", () => socket.write(bytes));
        socket.once("data", (chunk) => {
          resolve([...chunk]);
          socket.destroy();
        });
        socket.on("error", reject);
      }),
  );

const refusesConnections = (port: number) =>
  Effect.promise(
    () =>
      new Promise<boolean>((resolve) => {
        const socket = NodeNet.connect(port, "127.0.0.1", () => {
          socket.destroy();
          resolve(false);
        });
        socket.on("error", () => resolve(true));
      }),
  );

it.effect("forwards a port over the tunnel on the same local port until stopped", () =>
  Effect.gen(function* () {
    const endpoint = yield* listenEndpoint;
    const remotePort = yield* unusedPort;
    const tunnel = yield* makeTunnel(
      `ws://127.0.0.1:${addressPort(endpoint.server)}/api/preview/tunnel?wsTicket=t`,
    );
    const announced: number[][] = [];
    yield* tunnel.subscribeForwards((forwards) =>
      Effect.sync(() => announced.push(forwards.map((forward) => forward.localPort))),
    );
    const environmentId = EnvironmentId.make("remote");
    const forward = yield* tunnel.forward({ environmentId, remotePort });
    expect(forward).toEqual({ environmentId, remotePort, localPort: remotePort });

    expect(yield* exchange(remotePort, Uint8Array.of(4, 5))).toEqual([0xee, 4, 5]);
    // The first request is the same-machine probe.
    expect(endpoint.requests.at(-1)).toBe(`/api/preview/tunnel?wsTicket=t&port=${remotePort}`);

    yield* tunnel.stopForward({ environmentId, remotePort });
    expect(yield* refusesConnections(remotePort)).toBe(true);
    expect(yield* tunnel.forwards).toEqual([]);
    expect(announced).toEqual([[remotePort], []]);
  }),
);

it.effect("moves a forward to a free port when its own is taken here", () =>
  Effect.gen(function* () {
    const decoy = yield* listenDecoy;
    const endpoint = yield* listenEndpoint;
    const remotePort = addressPort(decoy.server);
    const tunnel = yield* makeTunnel(
      `ws://127.0.0.1:${addressPort(endpoint.server)}/api/preview/tunnel`,
    );
    const forward = yield* tunnel.forward({
      environmentId: EnvironmentId.make("remote"),
      remotePort,
    });
    expect(forward.localPort).not.toBe(remotePort);

    expect(yield* exchange(forward.localPort, Uint8Array.of(6))).toEqual([0xee, 6]);
    expect(endpoint.requests.at(-1)).toBe(`/api/preview/tunnel?port=${remotePort}`);
    expect(decoy.connections()).toBe(0);
  }),
);

/** An environment on this machine: each tunnel connection dials the requested local port. */
const listenSameMachineEndpoint = Effect.acquireRelease(
  Effect.promise(
    () =>
      new Promise<NodeWS.WebSocketServer>((resolve) => {
        const server: NodeWS.WebSocketServer = new NodeWS.WebSocketServer(
          { port: 0, host: "127.0.0.1" },
          () => resolve(server),
        );
        server.on("connection", (socket: NodeWS.WebSocket, request: { url?: string }) => {
          const port = Number(new URL(request.url ?? "", "ws://x").searchParams.get("port"));
          const local = NodeNet.connect(port, "127.0.0.1");
          local.on("data", (chunk) => socket.send(chunk));
          local.on("error", () => socket.close());
          socket.on("message", (data: Buffer) => local.write(data));
          socket.on("close", () => local.destroy());
        });
      }),
  ),
  closeServer,
);

it.effect("refuses to forward from an environment that shares this machine's loopback", () =>
  Effect.gen(function* () {
    const endpoint = yield* listenSameMachineEndpoint;
    const remotePort = yield* unusedPort;
    const tunnel = yield* makeTunnel(`ws://127.0.0.1:${addressPort(endpoint)}/api/preview/tunnel`);
    const error = yield* tunnel
      .forward({ environmentId: EnvironmentId.make("remote"), remotePort })
      .pipe(Effect.flip);
    expect(error._tag).toBe("LoopbackTunnelSameMachineError");
    expect(yield* refusesConnections(remotePort)).toBe(true);
    expect(yield* tunnel.forwards).toEqual([]);
  }),
);

it.effect(
  "forwards nothing while the environment is out of reach, then forwards once it is back",
  () =>
    Effect.gen(function* () {
      const endpoint = yield* listenEndpoint;
      const remotePort = yield* unusedPort;
      let access: string | null = null;
      const tunnel = yield* makeTunnel(() => access);
      const environmentId = EnvironmentId.make("remote");

      const unavailable = yield* tunnel.forward({ environmentId, remotePort }).pipe(Effect.flip);
      expect(unavailable._tag).toBe("LoopbackTunnelUnavailableError");
      expect(yield* refusesConnections(remotePort)).toBe(true);
      expect(yield* tunnel.forwards).toEqual([]);

      access = `ws://127.0.0.1:${addressPort(endpoint.server)}/api/preview/tunnel`;
      const forward = yield* tunnel.forward({ environmentId, remotePort });
      expect(yield* exchange(forward.localPort, Uint8Array.of(9))).toEqual([0xee, 9]);
    }),
);

/** The environment's tunnel route refusing every port with a 502, as for a port nothing listens on. */
const listenRefusingEndpoint = (headers: Record<string, string>) =>
  Effect.acquireRelease(
    Effect.promise(
      () =>
        new Promise<NodeWS.WebSocketServer>((resolve) => {
          const server: NodeWS.WebSocketServer = new NodeWS.WebSocketServer(
            {
              port: 0,
              host: "127.0.0.1",
              verifyClient: (_info, done) => done(false, 502, "Bad Gateway", headers),
            },
            () => resolve(server),
          );
        }),
    ),
    closeServer,
  );

it.effect.each([
  { case: "the environment reports nothing on the probe port", refused: true },
  { case: "the tunnel fails for another reason", refused: false },
])("decides a forward's machine only on proof: $case", ({ refused }) =>
  Effect.gen(function* () {
    const endpoint = yield* listenRefusingEndpoint(
      refused ? { "x-t3-tunnel-port-refused": "1" } : {},
    );
    const remotePort = yield* unusedPort;
    const tunnel = yield* makeTunnel(`ws://127.0.0.1:${addressPort(endpoint)}/api/preview/tunnel`);
    const result = yield* tunnel
      .forward({ environmentId: EnvironmentId.make("remote"), remotePort })
      .pipe(Effect.result);
    if (refused) {
      expect(Result.isSuccess(result)).toBe(true);
    } else {
      expect(Result.isFailure(result) && result.failure._tag).toBe(
        "LoopbackTunnelUnavailableError",
      );
      expect(yield* refusesConnections(remotePort)).toBe(true);
    }
  }),
);

it.effect("closes connections beyond a forward's limit without tunnelling them", () =>
  Effect.gen(function* () {
    const endpoint = yield* listenEndpoint;
    const tunnel = yield* makeTunnel(
      `ws://127.0.0.1:${addressPort(endpoint.server)}/api/preview/tunnel`,
    );
    const forward = yield* tunnel.forward({
      environmentId: EnvironmentId.make("remote"),
      remotePort: yield* unusedPort,
    });
    const open = () =>
      Effect.acquireRelease(
        Effect.promise(
          () =>
            new Promise<NodeNet.Socket>((resolve) => {
              const socket = NodeNet.connect(forward.localPort, "127.0.0.1", () => {
                socket.once("data", () => resolve(socket));
                socket.write(Uint8Array.of(1));
              });
            }),
        ),
        (socket) => Effect.sync(() => socket.destroy()),
      );
    yield* Effect.forEach(Array.from({ length: 64 }), open, { concurrency: "unbounded" });
    const tunnelled = endpoint.requests.length;
    const closed = yield* Effect.promise(
      () =>
        new Promise<boolean>((resolve) => {
          const socket = NodeNet.connect(forward.localPort, "127.0.0.1");
          socket.on("close", () => resolve(true));
          socket.on("data", () => resolve(false));
          socket.write(Uint8Array.of(2));
        }),
    );
    expect(closed).toBe(true);
    expect(endpoint.requests.length).toBe(tunnelled);
  }),
);

it.effect(
  "stops local port forwards on SSH disconnect and when dropped from the connection catalog",
  () =>
    Effect.gen(function* () {
      const endpoint = yield* listenEndpoint;
      const tunnel = yield* makeTunnel(
        `ws://127.0.0.1:${addressPort(endpoint.server)}/api/preview/tunnel`,
      );
      const announced: Array<ReadonlyArray<string>> = [];
      yield* tunnel.subscribeForwards((forwards) =>
        Effect.sync(() =>
          announced.push(
            forwards.map((forward) => `${forward.environmentId}:${forward.remotePort}`),
          ),
        ),
      );

      const env1 = EnvironmentId.make("ssh-env-1");
      const env2 = EnvironmentId.make("ssh-env-2");
      const sshTarget1 = {
        alias: "box-1",
        hostname: "box-1.example.test",
        username: "dev",
        port: 22,
      } as const;
      const sshTarget2 = {
        alias: "box-2",
        hostname: "box-2.example.test",
        username: "dev",
        port: 2222,
      } as const;
      const encodeCatalog = Schema.encodeEffect(Schema.fromJsonString(ConnectionCatalogDocument));
      const initialCatalog = yield* encodeCatalog({
        schemaVersion: 1,
        targets: [
          new SshConnectionTarget({
            environmentId: env1,
            label: "Box 1",
            connectionId: `ssh:${env1}`,
          }),
          new SshConnectionTarget({
            environmentId: env2,
            label: "Box 2",
            connectionId: `ssh:${env2}`,
          }),
        ],
        profiles: [
          new SshConnectionProfile({
            connectionId: `ssh:${env1}`,
            environmentId: env1,
            label: "Box 1",
            target: sshTarget1,
          }),
          new SshConnectionProfile({
            connectionId: `ssh:${env2}`,
            environmentId: env2,
            label: "Box 2",
            target: sshTarget2,
          }),
        ],
        credentials: [],
        remoteDpopTokens: [],
        disabledEnvironmentIds: [],
      });
      const catalogRef = yield* Ref.make<Option.Option<string>>(Option.some(initialCatalog));
      const catalogLayer = Layer.succeed(
        DesktopConnectionCatalogStore.DesktopConnectionCatalogStore,
        {
          get: Ref.get(catalogRef),
          set: (catalog) => Ref.set(catalogRef, Option.some(catalog)).pipe(Effect.as(true)),
        },
      );
      const sshLayer = Layer.succeed(DesktopSshEnvironment.DesktopSshEnvironment, {
        discoverHosts: () => Effect.succeed([]),
        resolveHost: () => Effect.die("unexpected resolveHost"),
        ensureEnvironment: () => Effect.die("unexpected ensureEnvironment"),
        disconnectEnvironment: () => Effect.void,
        closeAllTunnels: Effect.void,
      });
      const ipcLayers = Layer.mergeAll(
        Layer.succeed(LoopbackTunnel.LoopbackTunnel, tunnel),
        catalogLayer,
        sshLayer,
      );

      const port1 = yield* unusedPort;
      const port2 = yield* unusedPort;
      const port3 = yield* unusedPort;
      const fwd1 = yield* tunnel.forward({ environmentId: env1, remotePort: port1 });
      const fwd2 = yield* tunnel.forward({ environmentId: env1, remotePort: port2 });
      const fwd3 = yield* tunnel.forward({ environmentId: env2, remotePort: port3 });

      yield* disconnectSshEnvironment.handler(sshTarget1).pipe(Effect.provide(ipcLayers));

      expect(yield* refusesConnections(fwd1.localPort)).toBe(true);
      expect(yield* refusesConnections(fwd2.localPort)).toBe(true);
      expect(yield* refusesConnections(fwd3.localPort)).toBe(false);
      expect(yield* tunnel.forwards).toEqual([fwd3]);

      const catalogWithoutEnv2 = yield* encodeCatalog({
        schemaVersion: 1,
        targets: [
          new SshConnectionTarget({
            environmentId: env1,
            label: "Box 1",
            connectionId: `ssh:${env1}`,
          }),
        ],
        profiles: [
          new SshConnectionProfile({
            connectionId: `ssh:${env1}`,
            environmentId: env1,
            label: "Box 1",
            target: sshTarget1,
          }),
        ],
        credentials: [],
        remoteDpopTokens: [],
        disabledEnvironmentIds: [],
      });
      expect(
        yield* setConnectionCatalog.handler(catalogWithoutEnv2).pipe(Effect.provide(ipcLayers)),
      ).toBe(true);

      expect(yield* refusesConnections(fwd3.localPort)).toBe(true);
      expect(yield* tunnel.forwards).toEqual([]);
      expect(announced.at(-2)).toEqual([`${env2}:${port3}`]);
      expect(announced.at(-1)).toEqual([]);

      // Also stops forwards when a previously active environment becomes disabled in the catalog.
      const fwd3Reopened = yield* tunnel.forward({ environmentId: env2, remotePort: port3 });
      const catalogDisablingEnv2 = yield* encodeCatalog({
        schemaVersion: 1,
        targets: [
          new SshConnectionTarget({
            environmentId: env1,
            label: "Box 1",
            connectionId: `ssh:${env1}`,
          }),
          new SshConnectionTarget({
            environmentId: env2,
            label: "Box 2",
            connectionId: `ssh:${env2}`,
          }),
        ],
        profiles: [
          new SshConnectionProfile({
            connectionId: `ssh:${env1}`,
            environmentId: env1,
            label: "Box 1",
            target: sshTarget1,
          }),
          new SshConnectionProfile({
            connectionId: `ssh:${env2}`,
            environmentId: env2,
            label: "Box 2",
            target: sshTarget2,
          }),
        ],
        credentials: [],
        remoteDpopTokens: [],
        disabledEnvironmentIds: [env2],
      });
      yield* Ref.set(catalogRef, Option.some(initialCatalog));
      expect(
        yield* setConnectionCatalog.handler(catalogDisablingEnv2).pipe(Effect.provide(ipcLayers)),
      ).toBe(true);
      expect(yield* refusesConnections(fwd3Reopened.localPort)).toBe(true);
      expect(yield* tunnel.forwards).toEqual([]);
    }),
);

it.effect("closes a forwarded connection when writing over the tunnel WebSocket fails", () =>
  Effect.gen(function* () {
    const endpoint = yield* listenEndpoint;
    const tunnel = yield* makeTunnel(
      `ws://127.0.0.1:${addressPort(endpoint.server)}/api/preview/tunnel`,
    );
    const forward = yield* tunnel.forward({
      environmentId: EnvironmentId.make("remote"),
      remotePort: yield* unusedPort,
    });
    using _spy = {
      [Symbol.dispose]: () => vi.restoreAllMocks(),
    };
    const originalSend = NodeWS.WebSocket.prototype.send;
    vi.spyOn(NodeWS.WebSocket.prototype, "send").mockImplementation(function (
      this: NodeWS.WebSocket,
      data: Parameters<NodeWS.WebSocket["send"]>[0],
      optionsOrCallback?: unknown,
      maybeCallback?: unknown,
    ) {
      const callback =
        typeof optionsOrCallback === "function"
          ? (optionsOrCallback as (err?: Error) => void)
          : typeof maybeCallback === "function"
            ? (maybeCallback as (err?: Error) => void)
            : undefined;
      if (this.url.includes(`port=${forward.remotePort}`)) {
        queueMicrotask(() => callback?.(new Error("socket broken")));
        return;
      }
      return (originalSend as (...args: unknown[]) => void).apply(
        this,
        maybeCallback === undefined
          ? [data, optionsOrCallback]
          : [data, optionsOrCallback, maybeCallback],
      );
    });

    const closed = yield* Effect.promise(
      () =>
        new Promise<boolean>((resolve) => {
          const socket = NodeNet.connect(forward.localPort, "127.0.0.1", () => {
            socket.write(Uint8Array.of(1, 2, 3));
          });
          socket.on("close", () => resolve(true));
          socket.on("data", () => resolve(false));
        }),
    );
    expect(closed).toBe(true);
  }),
);

it.effect(
  "sendWebSocketChunks fails with SocketWriteError on non-OPEN, sync throw, or chunk error",
  () =>
    Effect.gen(function* () {
      const closedSend = vi.fn();
      const nonOpen = yield* LoopbackTunnel.sendWebSocketChunks(
        { readyState: 3, send: closedSend as unknown as NodeWS.WebSocket["send"] },
        [Uint8Array.of(1)],
      ).pipe(Effect.flip);
      expect(nonOpen.reason._tag).toBe("SocketWriteError");
      expect(closedSend).not.toHaveBeenCalled();

      const syncError = yield* LoopbackTunnel.sendWebSocketChunks(
        {
          readyState: 1,
          send: (() => {
            throw new Error("sync write failure");
          }) as unknown as NodeWS.WebSocket["send"],
        },
        [Uint8Array.of(1)],
      ).pipe(Effect.flip);
      expect(syncError.reason._tag).toBe("SocketWriteError");

      const callbackError = yield* LoopbackTunnel.sendWebSocketChunks(
        {
          readyState: 1,
          send: ((chunk: Uint8Array, cb?: (err?: Error) => void) => {
            queueMicrotask(() => cb?.(chunk[0] === 1 ? new Error("chunk 1 failed") : undefined));
          }) as unknown as NodeWS.WebSocket["send"],
        },
        [Uint8Array.of(1), Uint8Array.of(2)],
      ).pipe(Effect.flip);
      expect(callbackError.reason._tag).toBe("SocketWriteError");
    }),
);

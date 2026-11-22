// Unknown paths run every job; recognized packages pull in their dependents.
// scripts/check-changed.ts shares these decisions so local checks match CI.
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");

const INERT = [
  /^docs\//,
  /^\.(agents|cursor|legion-cli|vscode|devcontainer|macroscope|repos)\//,
  /^[^/]+\.md$/,
  /^LICENSE$/,
];

// Shared test setup and vendored mobile dependencies affect every package.
const RUNS_EVERYTHING = [
  /^packages\/shared\/src\/(HostProcess|testing\/(longTempDir|providerEnv|gitEnv|gitRepositoryEnv))\.ts$/,
  /^apps\/mobile\/deps\//,
];

// Includes cross-package reads absent from package.json dependencies.
const EXTRA_OWNERS = [
  // Vite configs and tsconfigs of these apps import scripts/lib directly.
  [/^scripts\/lib\//, ["@legioncode/desktop", "@legioncode/mobile", "@legioncode/web", "legioncode"]],
  // scripts/build-desktop-artifact.ts reads the GNOME extension manifest.
  [/^apps\/desktop\/gnome-extension\//, ["@legioncode/scripts"]],
  [/^native\/browser-secret\//, ["@legioncode/desktop"]],
  [/^native\/kde-snap-shot\//, ["@legioncode/desktop"]],
  [/^native\/libghostty-vt\//, ["@legioncode/web"]],
  // apps/server/src/cloud/ConnectReachability.test.ts checks the guide's headings.
  [/^docs\/user\/legion-connect-troubleshooting\.md$/, ["legioncode"]],
];
const RUST = /^(native\/(resource-monitor|kde-snap-shot|hyprland-snap-shot)|rust)\//;
const RUST_CI_TRIGGER = [
  RUST,
  /^\.cargo\//,
  /^(apps\/server\/src\/rustCutover\.ts|apps\/desktop\/scripts\/start-electron\.mjs|apps\/mobile\/scripts\/sync-rust-mobile-ffi\.mjs|packages\/client-runtime\/src\/wasm\/|infra\/(relay|task-cache)\/scripts\/stage-rust-worker\.mjs|scripts\/(build-cli-archive|smoke-cli-archive)\.ts)$/,
  /^\.github\/(workflows\/ci\.yml|actions\/ci-changes\/|scripts\/ci-changes\.cjs)$/,
];

// These tests read other packages without declaring dependencies.
const TEST_ONLY_DEPS = {
  // Evals import the server's source to test its real model-facing text.
  "@legioncode/evals": ["legioncode"],
  "@legioncode/provider-pi": ["@legioncode/client-runtime"],
  "@legioncode/scripts": [
    "@legioncode/desktop",
    "@legioncode/mobile",
    "@legioncode/web",
    "legioncode",
    "@legioncode/provider-cursor",
  ],
};
// The server bundles the web build without importing it.
const BUILD_ONLY_DEPS = { legioncode: ["@legioncode/web"] };

const MOBILE_NATIVE =
  /^apps\/mobile\/.*\.(swift|kt|kts)$|^apps\/mobile\/(\.swiftlint\.yml|detekt\.yml|\.editorconfig|Brewfile)$|^scripts\/mobile-native-static-check\.ts$|^package\.json$|^\.github\/workflows\/ci\.yml$|^\.github\/scripts\/ci-changes\.cjs$|^\.github\/actions\/ci-changes\//;

// Startup, migration, settlement, and sidebar paths that decide whether an upgraded user's
// threads show after launch. Build runs scripts/smoke-desktop-app.ts for them, so they do not
// first fail in the release launch check.
const DESKTOP_LAUNCH = [
  /^apps\/desktop\/src\/((main|boot|compileCache|serverCompileCache)\.ts$|(app|backend|window)\/)/,
  /^apps\/desktop\/src\/electron\/(ElectronApp|ElectronProtocol|ElectronWindow)\.ts$/,
  /^apps\/web\/src\/((main|AppRoot|appInit)\.tsx|(bootstrap|router)\.ts)$/,
  /^apps\/web\/src\/.*sidebar/i,
  /^apps\/server\/src\/(bin|binMain|server|serverRuntimeStartup)\.ts$/,
  /^apps\/server\/src\/persistence\/Migrations/,
  /^apps\/server\/src\/orchestration-v2\/((ThreadSettlementService|ProjectionStore)\.ts$|legacy\/)/,
  /^packages\/contracts\/src\/settings\.ts$/,
  /^scripts\/(smoke-desktop-app\.ts|fixtures\/desktop-launch\/)/,
];

const OWN_TEST_JOB = new Set(["legioncode", "@legioncode/web", "@legioncode/monorepo"]);

function readWorkspace() {
  const patterns = [];
  let inPackages = false;
  for (const line of fs.readFileSync(path.join(root, "pnpm-workspace.yaml"), "utf8").split("\n")) {
    if (/^\S/.test(line)) inPackages = line.startsWith("packages:");
    const entry = inPackages && line.match(/^\s+-\s+["']?([^"'\s]+)/);
    if (entry) patterns.push(entry[1]);
  }

  const dirs = patterns.flatMap((pattern) => {
    if (!pattern.endsWith("/*")) return [pattern];
    const parent = pattern.slice(0, -2);
    return fs.readdirSync(path.join(root, parent)).map((name) => `${parent}/${name}`);
  });

  const packages = [];
  for (const dir of dirs) {
    const manifestPath = path.join(root, dir, "package.json");
    if (!fs.existsSync(manifestPath)) continue;
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    const deps = {
      ...manifest.dependencies,
      ...manifest.devDependencies,
      ...manifest.peerDependencies,
    };
    packages.push({
      name: manifest.name,
      dir,
      hasTest: Boolean(manifest.scripts?.test),
      deps: Object.keys(deps).filter(
        (dep) =>
          String(deps[dep]).startsWith("workspace:") &&
          !BUILD_ONLY_DEPS[manifest.name]?.includes(dep),
      ),
    });
  }
  return packages;
}

// [] means inert; null means unknown and requires every job.
function ownersOf(file, packages) {
  const extra = EXTRA_OWNERS.flatMap(([pattern, names]) => (pattern.test(file) ? names : []));
  if (INERT.some((pattern) => pattern.test(file))) return extra;
  // A manifest can change dependencies or versions other packages read.
  if (path.posix.basename(file) === "package.json") return null;
  if (RUNS_EVERYTHING.some((pattern) => pattern.test(file))) return null;
  if (/^\.cargo\//.test(file)) return extra;
  // scripts/build-desktop-artifact.test.ts reads the crates' sources.
  if (RUST.test(file)) return ["@legioncode/scripts", ...extra];

  const owner = packages
    .filter((pkg) => file.startsWith(`${pkg.dir}/`))
    .sort((a, b) => b.dir.length - a.dir.length)[0];
  if (owner) return [owner.name, ...extra];
  return extra.length > 0 ? extra : null;
}

function decide(files, packages, full) {
  const all = new Set(packages.map((pkg) => pkg.name));
  let affected = new Set();
  let everything = full || files === null;
  let rust = false;

  for (const file of files ?? []) {
    if (RUST_CI_TRIGGER.some((pattern) => pattern.test(file))) rust = true;
    const owners = ownersOf(file, packages);
    if (owners === null) {
      everything = true;
      console.error(`Runs everything: ${file}`);
    } else {
      for (const owner of owners) affected.add(owner);
    }
  }

  // Release smoke reads scripts/ itself, not what the scripts tests read.
  let scriptsChanged = everything;
  if (everything) {
    affected = all;
  } else {
    let grew = true;
    while (grew) {
      grew = false;
      for (const pkg of packages) {
        if (!affected.has(pkg.name) && pkg.deps.some((dep) => affected.has(dep))) {
          affected.add(pkg.name);
          grew = true;
        }
      }
    }
    scriptsChanged = affected.has("@legioncode/scripts");
    for (const [name, deps] of Object.entries(TEST_ONLY_DEPS)) {
      if (deps.some((dep) => affected.has(dep))) affected.add(name);
    }
  }

  const tested = packages.filter(
    (pkg) => pkg.hasTest && !OWN_TEST_JOB.has(pkg.name) && affected.has(pkg.name),
  );
  const has = (name) => affected.has(name);
  const desktopLaunch =
    everything ||
    files.some(
      (file) => !/\.test\.tsx?$/.test(file) && DESKTOP_LAUNCH.some((pattern) => pattern.test(file)),
    );

  return {
    affected: [...affected].sort(),
    outputs: {
      typecheck: affected.size > 0,
      build: has("@legioncode/desktop") || has("legioncode") || has("@legioncode/web") || desktopLaunch,
      desktop_launch: desktopLaunch,
      test: tested.length > 0,
      // Empty means every package the Test job covers.
      test_packages: everything ? "" : tested.map((pkg) => pkg.name).join(" "),
      test_web: has("@legioncode/web"),
      test_server: has("legioncode"),
      rust: everything || rust,
      js_checks:
        everything ||
        affected.size > 0 ||
        Boolean(
          files?.some(
            (file) =>
              /^\.github\/(workflows|scripts|actions)\//.test(file) || file === "knip.jsonc",
          ),
        ),
      release_smoke: scriptsChanged,
      mobile_native: files === null || files.some((file) => MOBILE_NATIVE.test(file)),
    },
  };
}

function main() {
  const args = process.argv.slice(2);
  const pathsIndex = args.indexOf("--paths");
  const files =
    pathsIndex === -1
      ? null
      : fs
          .readFileSync(args[pathsIndex + 1], "utf8")
          .split("\n")
          .filter(Boolean);
  const { affected, outputs } = decide(files, readWorkspace(), args.includes("--full"));

  console.error(`Affected packages: ${affected.join(", ") || "none"}`);
  for (const [key, value] of Object.entries(outputs)) console.log(`${key}=${value}`);
}

module.exports = { readWorkspace, decide };

if (require.main === module) main();

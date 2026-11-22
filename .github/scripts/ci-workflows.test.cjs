const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const { decide, readWorkspace } = require("./ci-changes.cjs");

const repoRoot = path.resolve(__dirname, "../..");
const workflowsDir = path.join(repoRoot, ".github", "workflows");
const actionsDir = path.join(repoRoot, ".github", "actions");

function listActionFiles() {
  const workflowFiles = fs
    .readdirSync(workflowsDir)
    .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
    .sort()
    .map((name) => path.join(workflowsDir, name));

  const actionFiles = fs
    .readdirSync(actionsDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => path.join(actionsDir, entry.name, "action.yml"))
    .filter((filePath) => fs.existsSync(filePath))
    .sort();

  return [...workflowFiles, ...actionFiles];
}

test("all workflows and actions use standard runners, fork-safe guards, and SHA-pinned external actions", () => {
  const forbidden = [
    "blacksmith-",
    "useblacksmith",
    "stickydisk",
    ["ping", "dotgg/t3", "code"].join(""),
    ["T3", "_TASK_CACHE_READ_TOKEN"].join(""),
    ["@t3", "tools"].join(""),
    ["T3", "CODE_"].join(""),
    "ubuntu-latest",
  ];

  const externalUses = /^\s*(?:-\s*)?uses:\s*([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+)@([^\s#]+)/gm;
  const shaPattern = /^[0-9a-f]{40}$/;

  for (const filePath of listActionFiles()) {
    const rel = path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
    const contents = fs.readFileSync(filePath, "utf8");

    for (const token of forbidden) {
      assert.equal(
        contents.includes(token),
        false,
        `${rel} must not contain legacy token '${token}'`,
      );
    }

    for (const match of contents.matchAll(externalUses)) {
      const [, action, ref] = match;
      assert.match(
        ref,
        shaPattern,
        `${rel}: action '${action}@${ref}' must be pinned to a 40-char commit SHA`,
      );
    }

    for (const line of contents.split(/\r?\n/u)) {
      if (/^    if:/.test(line)) {
        assert.doesNotMatch(
          line,
          /\bsecrets\./,
          `${rel}: job-level if must not reference secrets (${line.trim()})`,
        );
      }
    }
  }
});

test("ci.yml enforces Sovereign Rust gates, dist/smoke/cutover, and Check branch protection job", () => {
  const ci = fs.readFileSync(path.join(workflowsDir, "ci.yml"), "utf8");

  assert.match(ci, /^\s+name:\s*Check\s*$/m);
  for (const required of [
    "node rust/scripts/check-rust-gates.mjs",
    "cargo fmt --manifest-path rust/Cargo.toml --all --check",
    "node rust/scripts/build-dist.mjs",
    "node rust/scripts/smoke-wasm-surfaces.mjs",
    "bash rust/cutover.sh verify",
    "wasm-bindgen-cli@0.2.130",
    "ubuntu-24.04",
    "macos-15",
    "windows-2025",
  ]) {
    assert.ok(ci.includes(required), `ci.yml must include '${required}'`);
  }
});

test("release.yml covers all six Sovereign Rust targets", () => {
  const release = fs.readFileSync(path.join(workflowsDir, "release.yml"), "utf8");

  for (const targetKey of [
    "linux-x64",
    "linux-arm64",
    "darwin-arm64",
    "darwin-x64",
    "win32-x64",
    "win32-arm64",
  ]) {
    assert.ok(release.includes(`platform_key: ${targetKey}`), `release.yml missing ${targetKey}`);
  }
  assert.ok(release.includes("SHA256SUMS"));
});

test("ci-changes.cjs routes Rust, cutover bridge, CI, and inert changes accurately", () => {
  const packages = readWorkspace();

  const docsOnly = decide(["docs/guide.md", ".legion-cli/notes.md", "README.md"], packages, false);
  assert.equal(docsOnly.outputs.rust, false);
  assert.equal(docsOnly.outputs.js_checks, false);
  assert.equal(docsOnly.outputs.typecheck, false);

  const rustCargo = decide([".cargo/config.toml"], packages, false);
  assert.equal(rustCargo.outputs.rust, true);
  assert.equal(rustCargo.outputs.typecheck, false);

  const cutoverBridge = decide(["apps/server/src/rustCutover.ts"], packages, false);
  assert.equal(cutoverBridge.outputs.rust, true);
  assert.equal(cutoverBridge.outputs.js_checks, true);
  assert.equal(cutoverBridge.outputs.typecheck, true);

  const ciWorkflow = decide([".github/workflows/ci.yml"], packages, false);
  assert.equal(ciWorkflow.outputs.rust, true);
  assert.equal(ciWorkflow.outputs.js_checks, true);
});

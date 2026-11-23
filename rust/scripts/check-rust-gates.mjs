#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const rustRoot = path.resolve(scriptDir, "..");
const cratesDir = path.join(rustRoot, "crates");

const FFI_CRATES = new Set(["legion-process-ffi", "legion-wasm-ffi", "legion-mobile-ffi"]);
const WARN_LOC_THRESHOLD = 400;
const HARD_LOC_LIMIT = 600;

function listRsFiles(dir) {
  const results = [];
  if (!fs.existsSync(dir)) return results;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...listRsFiles(full));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      results.push(full);
    }
  }
  return results;
}

function stripStringsAndComments(line) {
  return line
    .replace(/\/\/.*$/u, "")
    .replace(/"(?:\\.|[^"\\])*"/gu, '""');
}

const crates = fs
  .readdirSync(cratesDir, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort();

if (crates.length < 19) {
  throw new Error(`Expected at least 19 crates under rust/crates, found ${crates.length}`);
}

const errors = [];
const warnings = [];
let totalFiles = 0;
let maxLines = 0;
let maxLineFile = "";

for (const crateName of crates) {
  const cratePath = path.join(cratesDir, crateName);
  const isFfi = FFI_CRATES.has(crateName);
  const roots = ["src/lib.rs", "src/main.rs"]
    .map((rel) => path.join(cratePath, rel))
    .filter((filePath) => fs.existsSync(filePath));

  if (roots.length === 0) {
    errors.push(`${crateName}: missing src/lib.rs and src/main.rs`);
  }

  for (const rootFile of roots) {
    const text = fs.readFileSync(rootFile, "utf8");
    const rel = path.relative(rustRoot, rootFile).split(path.sep).join("/");
    if (isFfi) {
      if (!/#!\[deny\(unsafe_op_in_unsafe_fn\)\]/u.test(text)) {
        errors.push(`${rel}: quarantined FFI crate must declare #![deny(unsafe_op_in_unsafe_fn)]`);
      }
    } else if (!/#!\[forbid\(unsafe_code\)\]/u.test(text)) {
      errors.push(`${rel}: pure crate must declare #![forbid(unsafe_code)]`);
    }
  }

  const allRsFiles = listRsFiles(cratePath);
  const srcRsFiles = listRsFiles(path.join(cratePath, "src"));
  const exampleRsFiles = listRsFiles(path.join(cratePath, "examples"));

  for (const exFile of exampleRsFiles) {
    const text = fs.readFileSync(exFile, "utf8");
    if (!/#!\[forbid\(unsafe_code\)\]/u.test(text)) {
      errors.push(
        `${path.relative(rustRoot, exFile).split(path.sep).join("/")}: example must declare #![forbid(unsafe_code)]`,
      );
    }
  }

  for (const filePath of allRsFiles) {
    totalFiles += 1;
    const rel = path.relative(rustRoot, filePath).split(path.sep).join("/");
    const text = fs.readFileSync(filePath, "utf8");
    const lines = text.split(/\r?\n/u);
    // Do not count a trailing newline-terminated empty slice if file ends with \n.
    const physicalLines = lines.length > 0 && lines[lines.length - 1] === "" ? lines.length - 1 : lines.length;
    if (physicalLines > maxLines) {
      maxLines = physicalLines;
      maxLineFile = rel;
    }
    if (physicalLines > HARD_LOC_LIMIT) {
      errors.push(`${rel}: ${physicalLines} lines exceeds hard ceiling of ${HARD_LOC_LIMIT}`);
    } else if (physicalLines > WARN_LOC_THRESHOLD) {
      warnings.push(`${rel}: ${physicalLines} lines exceeds target of ${WARN_LOC_THRESHOLD}`);
    }

    for (let i = 0; i < lines.length; i += 1) {
      const stripped = stripStringsAndComments(lines[i]);
      if (/#\[\s*allow\s*\(/u.test(stripped)) {
        errors.push(`${rel}:${i + 1}: uses #[allow(...)] instead of #[expect(..., reason = "...")]`);
      }
    }
  }

  for (const filePath of srcRsFiles) {
    const rel = path.relative(rustRoot, filePath).split(path.sep).join("/");
    const lines = fs.readFileSync(filePath, "utf8").split(/\r?\n/u);

    for (let i = 0; i < lines.length; i += 1) {
      const line = lines[i];
      const stripped = stripStringsAndComments(line);
      if (/\b(?:todo|unimplemented)\s*!\s*\(/u.test(stripped)) {
        errors.push(`${rel}:${i + 1}: contains todo!() or unimplemented!() stub`);
      }

      if (/^#!\[(?:forbid|deny)\(/u.test(stripped.trim()) || /^#\[unsafe\(/u.test(stripped.trim())) {
        continue;
      }

      const hasUnsafeKeyword = /\bunsafe\s*(?:\{|fn\b|extern\b|impl\b|trait\b)/u.test(stripped);
      if (!hasUnsafeKeyword) continue;

      if (!isFfi) {
        errors.push(`${rel}:${i + 1}: pure crate contains unsafe syntax`);
      } else {
        const windowStart = Math.max(0, i - 5);
        const hasSafety = lines
          .slice(windowStart, i + 1)
          .some((candidate) => /\/\/\s*SAFETY:/u.test(candidate));
        if (!hasSafety) {
          errors.push(`${rel}:${i + 1}: unsafe syntax missing nearby '// SAFETY:' invariant comment`);
        }
      }
    }
  }
}

for (const warning of warnings) {
  process.stderr.write(`[rust-gates] WARN: ${warning}\n`);
}

if (errors.length > 0) {
  for (const err of errors) {
    process.stderr.write(`[rust-gates] FAIL: ${err}\n`);
  }
  process.exit(1);
}

process.stdout.write(
  JSON.stringify({
    passed: true,
    crates: crates.length,
    pureCrates: crates.length - FFI_CRATES.size,
    ffiCrates: FFI_CRATES.size,
    rsFiles: totalFiles,
    maxPhysicalLines: maxLines,
    maxLineFile,
  }) + "\n",
);

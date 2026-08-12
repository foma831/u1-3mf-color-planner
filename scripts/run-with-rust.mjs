#!/usr/bin/env node

import { accessSync, constants } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { spawn, spawnSync } from "node:child_process";

const [, , command, ...args] = process.argv;

if (!command) {
  console.error("Usage: node scripts/run-with-rust.mjs <command> [args...]");
  process.exit(2);
}

function isExecutable(path) {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function executableName(name) {
  return process.platform === "win32" ? `${name}.exe` : name;
}

function findOnPath(name) {
  const executable = executableName(name);
  for (const directory of (process.env.PATH ?? "").split(delimiter)) {
    if (!directory) continue;
    const candidate = join(directory, executable);
    if (isExecutable(candidate)) return candidate;
  }
  return null;
}

function cargoFromRustup() {
  const rustup = findOnPath("rustup");
  if (!rustup) return null;

  const result = spawnSync(rustup, ["which", "cargo"], {
    cwd: process.cwd(),
    encoding: "utf8",
  });
  const candidate = result.status === 0 ? result.stdout.trim() : "";
  return candidate && isExecutable(candidate) ? candidate : null;
}

function findCargo() {
  const fromRustup = cargoFromRustup();
  if (fromRustup) return fromRustup;

  const candidates = [
    findOnPath("cargo"),
    join(process.env.CARGO_HOME ?? join(homedir(), ".cargo"), "bin", executableName("cargo")),
    join("/opt/homebrew/opt/rustup/bin", executableName("cargo")),
    join("/usr/local/opt/rustup/bin", executableName("cargo")),
  ];
  return candidates.find((candidate) => candidate && isExecutable(candidate)) ?? null;
}

const cargo = findCargo();
if (!cargo) {
  console.error(
    "Rust/Cargo was not found. Install rustup, then run `rustup toolchain install stable`, and retry.",
  );
  process.exit(1);
}

const cargoDirectory = dirname(cargo);
const env = {
  ...process.env,
  CARGO_INCREMENTAL: process.env.CARGO_INCREMENTAL ?? "0",
  PATH: [cargoDirectory, process.env.PATH].filter(Boolean).join(delimiter),
};
const executable = command === "cargo" ? cargo : command;
const child = spawn(executable, args, {
  cwd: process.cwd(),
  env,
  stdio: "inherit",
  shell: process.platform === "win32",
});

child.on("error", (error) => {
  console.error(`Failed to start ${command}: ${error.message}`);
  process.exitCode = 1;
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exitCode = code ?? 1;
});

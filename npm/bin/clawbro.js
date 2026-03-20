#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const binaryName = process.platform === "win32" ? "clawbro.exe" : "clawbro";
const binaryPath = path.join(__dirname, "..", "vendor", "bin", binaryName);

if (!fs.existsSync(binaryPath)) {
  console.error(
    "clawbro binary is not installed. Reinstall the package or run with CLAWBRO_SKIP_DOWNLOAD=0."
  );
  process.exit(1);
}

const result = spawnSync(binaryPath, process.argv.slice(2), {
  stdio: "inherit"
});

if (result.error) {
  console.error(`failed to launch clawbro: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status ?? 0);

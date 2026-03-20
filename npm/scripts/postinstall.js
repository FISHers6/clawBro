"use strict";

const fs = require("node:fs");
const path = require("node:path");
const https = require("node:https");
const { execFileSync } = require("node:child_process");

const pkg = require("../package.json");
const { releaseUrl, resolvePlatformArtifact } = require("./platform");

if (process.env.CLAWBRO_SKIP_DOWNLOAD === "1") {
  console.log("Skipping clawbro binary download because CLAWBRO_SKIP_DOWNLOAD=1");
  process.exit(0);
}

async function download(url, destination) {
  await new Promise((resolve, reject) => {
    const file = fs.createWriteStream(destination);
    https
      .get(url, (response) => {
        if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
          file.close();
          fs.rmSync(destination, { force: true });
          download(response.headers.location, destination).then(resolve).catch(reject);
          return;
        }
        if (response.statusCode !== 200) {
          reject(new Error(`download failed with status ${response.statusCode} for ${url}`));
          return;
        }
        response.pipe(file);
        file.on("finish", () => {
          file.close();
          resolve();
        });
      })
      .on("error", (error) => {
        file.close();
        reject(error);
      });
  });
}

async function main() {
  const { artifact, binaryName } = resolvePlatformArtifact();
  const url = releaseUrl(pkg.version, artifact);
  const rootDir = path.join(__dirname, "..");
  const tempDir = path.join(rootDir, ".download");
  const archivePath = path.join(tempDir, artifact);
  const unpackDir = path.join(tempDir, "unpacked");
  const vendorDir = path.join(rootDir, "vendor", "bin");
  const binaryPath = path.join(vendorDir, binaryName);

  fs.rmSync(tempDir, { recursive: true, force: true });
  fs.mkdirSync(tempDir, { recursive: true });

  console.log(`Downloading clawbro ${pkg.version} from ${url}`);
  await download(url, archivePath);

  fs.rmSync(unpackDir, { recursive: true, force: true });
  fs.mkdirSync(unpackDir, { recursive: true });
  execFileSync("tar", ["-xzf", archivePath, "-C", unpackDir], { stdio: "inherit" });

  const unpackedBinary = path.join(unpackDir, binaryName);
  if (!fs.existsSync(unpackedBinary)) {
    throw new Error(`expected binary ${binaryName} not found in ${artifact}`);
  }

  fs.mkdirSync(vendorDir, { recursive: true });
  fs.copyFileSync(unpackedBinary, binaryPath);
  fs.chmodSync(binaryPath, 0o755);
  fs.rmSync(tempDir, { recursive: true, force: true });

  console.log(`Installed clawbro binary to ${binaryPath}`);
}

main().catch((error) => {
  console.error(`clawbro postinstall failed: ${error.message}`);
  process.exit(1);
});

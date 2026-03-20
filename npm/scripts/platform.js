"use strict";

const REPO_OWNER = process.env.CLAWBRO_RELEASE_OWNER || "FISHers6";
const REPO_NAME = process.env.CLAWBRO_RELEASE_REPO || "clawBro";

function resolvePlatformArtifact(platform = process.platform, arch = process.arch) {
  if (platform === "darwin" && arch === "arm64") {
    return {
      artifact: "clawbro-darwin-aarch64.tar.gz",
      binaryName: "clawbro"
    };
  }
  if (platform === "darwin" && arch === "x64") {
    return {
      artifact: "clawbro-darwin-x86_64.tar.gz",
      binaryName: "clawbro"
    };
  }
  if (platform === "linux" && arch === "x64") {
    return {
      artifact: "clawbro-linux-x86_64.tar.gz",
      binaryName: "clawbro"
    };
  }
  throw new Error(`unsupported platform/arch for clawbro: ${platform}/${arch}`);
}

function releaseUrl(version, artifact) {
  const base =
    process.env.CLAWBRO_RELEASE_BASE_URL ||
    `https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/v${version}`;
  return `${base}/${artifact}`;
}

module.exports = {
  REPO_OWNER,
  REPO_NAME,
  releaseUrl,
  resolvePlatformArtifact
};

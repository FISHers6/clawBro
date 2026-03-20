# clawbro npm package

This package installs a prebuilt `clawbro` binary from GitHub Releases.

## Install

```bash
npm i -g clawbro
clawbro --version
```

## Notes

- Rust is not compiled locally during install.
- Supported in phase 1:
  - macOS arm64
  - macOS x64
  - Linux x64
- Set `CLAWBRO_SKIP_DOWNLOAD=1` to skip the postinstall downloader during local packaging tests.

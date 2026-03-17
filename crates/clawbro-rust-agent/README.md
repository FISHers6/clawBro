# clawbro-rust-agent

`clawbro-rust-agent` is the thin shell package built on top of `clawbro-agent-sdk`.

It supports:

- ACP shell mode
- native `--runtime-bridge` stdio mode

The reusable execution core lives in `clawbro-agent-sdk`; this package is the CLI and transport wrapper.

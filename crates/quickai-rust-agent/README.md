# quickai-rust-agent

`quickai-rust-agent` is the thin shell package built on top of `quickai-agent-sdk`.

It supports:

- ACP shell mode
- native `--runtime-bridge` stdio mode

The reusable execution core lives in `quickai-agent-sdk`; this package is the CLI and transport wrapper.

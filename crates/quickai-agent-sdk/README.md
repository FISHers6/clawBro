# quickai-agent-sdk

`quickai-agent-sdk` is the reusable QuickAI execution core.

It provides:

- provider/model configuration
- execution engine
- native runtime bridge logic
- tool registration and team-tool client wiring

It depends on `qai-protocol` for host-neutral contract types and is intended to support both library embedding and thin shell binaries.

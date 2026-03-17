# clawbro-agent-sdk

`clawbro-agent-sdk` is the reusable ClawBro execution core.

It provides:

- provider/model configuration
- execution engine
- native runtime bridge logic
- tool registration and team-tool client wiring

It depends on `clawbro-protocol` for host-neutral contract types and is intended to support both library embedding and thin shell binaries.

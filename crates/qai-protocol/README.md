# qai-protocol

`qai-protocol` defines the host-neutral runtime contract shared by QuickAI runtimes, shells, and gateway adapters.

It owns stable execution payloads such as:

- turn requests
- runtime events
- turn results
- approval and team-tool wire payloads

It does not own gateway-specific launch policy, backend routing, or resume bookkeeping.

# Client Integration Guides

Step-by-step instructions for A2A clients joining this switchboard. Each
guide covers registration, discovery, outbound calls, caller attribution,
and (where relevant) the reverse channel.

> These guides are generic on purpose. Replace `<GATEWAY_ORIGIN>`,
> `<TOKEN>`, `<NAME>`, `<URL>` with your actual values. Never commit real
> gateway origins, tokens, or LAN IPs.

## Index

| Client | Guide | Notes |
|---|---|---|
| Hermes agent runtime | [hermes.md](hermes.md) | Python; directory discovery + per-peer caller token. |
| Hermes agent delegation | [hermes-delegation-runbook.md](hermes-delegation-runbook.md) | Operational: toolset/consent config, task ladder, verification, pitfalls (verified live 2026-08-16). |
| Pi coding agent (pi-a2a) | [pi-coding-agent.md](pi-coding-agent.md) | Native gateway support: auto-register + reverse channel. |
| Any A2A client | [general.md](general.md) | Plain HTTP/JSON-RPC — works for every A2A-compatible client. |

For the full wire protocol, see [../INTEGRATION.md](../INTEGRATION.md).
For AI-agent self-onboarding, see
[../AGENTS-ONBOARDING.md](../AGENTS-ONBOARDING.md).
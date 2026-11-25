# Provider feature audit

Audited 2026-10-15 against official documentation, installed CLI help, the current Rust transports and protocol fixtures. A native CLI feature being inherited from its configuration is different from having a dedicated Code control. This inventory records both. It does not claim complete parity with every product release or successful inference on accounts that cannot currently run a model.

## Changes implemented from this audit

Cursor previously ran in print mode, which could not deliver its interactive approval requests to Code. It now uses its installed `cursor-agent acp` transport through the existing bounded Rust ACP worker. There is no embedded browser or JavaScript SDK in this integration.

- Native new/resume sessions, negotiated model choices, per-run model selection and the actual `agent`/`plan` modes.
- Provider-owned tool approvals, declines and cancellation with exact request IDs; unexpected filesystem/terminal client requests are explicitly rejected because Code does not advertise those client capabilities.
- Cursor's blocking multiple-choice questions and plan decisions use Code's native request tray. Unknown option IDs, duplicate IDs, mismatched answers and oversized requests are rejected. A read-only invocation cannot approve an implementation plan.
- Cursor child streaming is explicitly negotiated. Child identities must descend from a known root/child session; up to 64 are tracked. Child text appears in child cards rather than becoming the parent's response. Child tool approvals retain the same live request routing. Child previews and tracked state are bounded; Code does not claim a complete persistent child transcript viewer.
- Cursor task, todo and generated-image notifications appear as typed status/delegation cards. Todo patches preserve merge/replacement behavior. Image notifications expose provider-returned paths; they do not invent generation results or reimplement the provider's image service.
- Cursor's native `/` commands are discovered and revalidated from the live provider catalog. Unknown commands are not forwarded as paid model prose. Resumed ACP sessions now retain their command catalog while suppressing replayed historical messages.
- Standard ACP context/cost notifications now reach Code's usage UI for both Cursor and Gemini. Only explicitly reported USD costs are labeled dollars; missing token counts remain unknown.
- Existing credentials are reused. Code never invokes Cursor's `authenticate` method merely to probe capability: that method can open a browser login flow when signed out. The existing explicit login control remains the entry point.

## Verified evidence

The installed Cursor CLI (2026.10.14-941492e) exposes `acp --help`. A real, no-inference `initialize` request returned ACP v1, native session loading, image prompt support and a session-list capability. A no-inference new-session request correctly reported authentication required on this installation; no live Cursor model execution is claimed. Installed public transport code was inspected to verify the subagent capability flag and child event fields; no credential files or account identifiers were read or printed.

The Rust fixtures cover a production-dispatch resume followed by a native permission, user question, plan decision and child permission; native catalog/model selection; explicit unsupported-client errors; child completion; todo/image/context notifications; old-history suppression; answer validation; read-only plan enforcement; bounded child lineage; and currency handling. Existing Gemini permission, command, cancellation and large-details fixtures continue to apply to the shared worker.

## Feature families still requiring work

| Provider | Features preserved through the current bridge | Remaining dedicated Code integration |
|---|---|---|
| Claude Code | Existing CLI-owned rules, hooks, skills, plugins and MCP configuration; native bidirectional tool approval/questions; session resume/fork; reported usage/models/commands; child cards and cancellation | Full plugin/hook/MCP management, account-dependent tool execution verification, persistent background workers/teams, full nested child transcripts, structured output/budget/effort controls, checkpoints and remote/cloud control. Newer CLI fields need versioned fixtures rather than assuming this installed release implements them. |
| Gemini CLI | ACP sessions, modes, models, advertised commands, native permissions, image capability gating, cancellation and reported ACP usage | Dedicated custom-agent definition/selection UI, nested subagent lifecycle/transcripts, A2A remote agents, configuration-option controls, checkpoint restoration, extension/hook/policy editors, native audio/media input and persistent terminal services. A CLI's available subagent tools do not imply Code already renders every child lifecycle. |
| OpenCode | Private authenticated server, native prompt/resume/fork, commands, approvals/questions, model catalog, child session events, usage and project-scoped policy | Primary/custom agent picker and definition editor; full revert/unrevert/checkpoint UI; MCP OAuth/config management; dynamic custom-tool/schema controls; persistent server-owned child/terminal handling after the parent invocation ends; remote/shared session workflows. Existing native commands may expose some behavior without a dedicated UI. |
| Cursor | Native ACP integration described above; existing rules/MCP configuration remains CLI-owned | Native history discovery/list UI, persistent sessions/background terminals, explicit custom agent/model-parameter controls, full plugin/MCP management, cloud workers/automations/team switching, long-lived child controls and full editor/LSP integration. Cursor documents that team-dashboard MCP servers are unavailable over ACP. |
| Antigravity | Headless streaming, project policy/auth reuse, native model argument/resume and typed basic statuses | Interactive approval parity, native artifacts/browser recordings, full custom-agent/team/background progress, checkpoint/rewind controls, configurable effort/goal/schedules, plugins/MCP/sidecars and remote-control flows. Its TUI commands are not automatically supported headless protocol operations. |
| Grok Build | Headless response/tool/child parsing, existing configuration/auth, model selection and session handling | Native bidirectional permission transport, session dashboard/history control, custom-agent editor, reasoning/budget/schema controls, sandbox profile UI, memory/plugins/MCP management, worktree/environment lifecycle and leader/remote-worker controls. Installed help advertises these families; Code does not claim to expose them all. |

## Changes requiring a concrete choice or separate authorization

These are not silently enabled during an audit:

- Installing/updating providers, plugins or MCP servers, enabling hooks or sidecars, or editing shared provider configuration can execute code or change other clients' behavior.
- Connecting new accounts, launching browser OAuth, changing an active team, enabling external app tools or remote workers affects account/workspace access.
- Public session sharing, cloud jobs, paid inference, remote control, automatic scheduling and publishing require explicit intent and appropriate scopes.
- Restore/revert/reset actions can change files. A native review/preview and clear target are needed before mutation.
- Broader sandbox permissions, persistent allow rules and automatic approval must remain explicit decisions. A feature-rich UI must not silently broaden the provider's security policy.

## Primary sources

- [Cursor ACP](https://cursor.com/docs/cli/acp): sessions, permission outcomes, modes and Cursor extension request/notification schemas. Installed ACP help and local public implementation additionally verify this release's subagent negotiation.
- [Cursor MCP](https://cursor.com/docs/cli/mcp) and [Cursor CLI overview](https://cursor.com/docs/cli/overview): configured tool integration versus editor/cloud services.
- [ACP prompt-turn protocol](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/v1/prompt-turn.mdx): authoritative session update and context/currency semantics.
- [Claude programmatic use](https://code.claude.com/docs/en/headless), [CLI reference](https://code.claude.com/docs/en/cli-reference) and [subagents](https://code.claude.com/docs/en/sub-agents): CLI/SDK differences, lifecycle and account/version-dependent features.
- [Gemini ACP](https://geminicli.com/docs/cli/acp-mode/), [subagents](https://geminicli.com/docs/core/subagents/), [commands](https://geminicli.com/docs/cli/commands/), [hook reference](https://geminicli.com/docs/hooks/reference/) and [configuration](https://geminicli.com/docs/reference/configuration/).
- [OpenCode server](https://opencode.ai/docs/server/), [agents](https://opencode.ai/docs/agents/), [permissions](https://opencode.ai/docs/permissions/) and [tools](https://opencode.ai/docs/tools/): native API versus interactive/editor controls.
- [Antigravity headless](https://antigravity.google/docs/cli/headless) and [CLI reference](https://antigravity.google/docs/cli/reference): headless event model versus interactive feature families.
- [Grok Build source repository](https://github.com/xai-org/grok-build) and installed `grok --help` / `.build/cli-reference/agent.txt`: the installed command surface is stronger evidence than assuming every public branch matches the local binary.

For Codex's complete installed protocol inventory and shared limitations, see [FEATURE-COVERAGE.md](FEATURE-COVERAGE.md). App-level product comparisons and project-management work are recorded separately.

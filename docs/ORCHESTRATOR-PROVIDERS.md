# Workspace orchestration across providers

Orchestrate is a mode of the currently selected chat. Enabling it preserves the
provider, exact model selection, thinking level, Ninja route, workspace, draft,
and conversation identity. It is available in both Code and the extended notch.
Click again to turn it off. Each submitted turn captures its mode; changing the
button during a running turn applies to the next prompt. Ordinary new chats do
not inherit orchestration mode.

Codex (including Ninja with the Codex harness) uses its native dynamic tools.
Claude and Ninja's Claude harness load the same tool schemas through a stdio MCP
server. Gemini, Cursor, and Grok receive it in ACP session creation and loading.
OpenCode receives an additional per-process MCP config. The selected native
harness still owns its authentication, model, execution, and approvals.

Antigravity does not offer a session-specific MCP flag. Kai merges a single
`kai_workspace` entry into its native MCP config, preserving other entries. This
helper advertises no tools outside a Kai invocation. Its private route is passed
only in the child process environment, never saved in the global registration.
An empty config file is initialized; malformed existing nonempty JSON is not
replaced. The managed helper path can be updated when installing a newer Kai.

The bridge uses a private Unix socket and an unpredictable per-turn token. It
allows eight concurrent calls, bounded requests/replies, and a 90-second reply
deadline. Both transport and pending replies stop with the owning run. It creates
no listener, helper process, or polling timer for ordinary non-orchestrator chats.
The helper cannot itself execute project operations. Calls enter the existing
workspace dispatcher, which validates the live owner and captures worker
permissions through the existing durable scheduler. Read-only mode and sandbox
policy are not broadened by enabling orchestration.

OpenCode orchestrator processes are scoped to their invocation so a reused
server cannot expose a previous chat's MCP route. The native session is resumed
on subsequent prompts. Native background subagents keep their normal service
lifetime and settlement behavior. OpenCode's full provider catalog has a separate 16 MiB
bound; conversation and other metadata responses retain the 2 MiB bound.

New workers continue to use the provider explicitly requested by the controller,
or the established Codex default. Existing worker chats retain their identity.
Models are inherited from a controller only when the worker uses the same
provider; cross-provider workers use their own provider selection. Explicit
models and thinking levels still require the actual `list_models` catalog.

## Verification

`cargo test --offline --manifest-path integrations/Code/Cargo.toml --bin orbit orchestrator`
checks preservation of all eight provider selections, existing chat toggling,
worker model isolation, task receipts, permissions, and workspace dispatch.

`cargo test --offline --manifest-path integrations/Code/Cargo.toml --bin orbit runtime::`
checks native provider protocols, MCP configuration/round trips, cancellation,
stale calls, and bounded metadata behavior. Protocol fixtures execute the actual
MCP helper for Claude and all three ACP providers, on both new and resumed chats.

For an explicit installed-provider check, run the ignored
`runtime::workspace_mcp::tests::installed_provider_calls_workspace_tools_without_switching_provider`
test with `KAI_ORCHESTRATOR_TEST_PROVIDER` set to a provider id (`claude`, `gemini`, `cursor`, `grok`, `opencode`,
`antigravity`). It uses
a temporary project, requests only `list_projects`, returns a fixture result from
the UI side, and verifies the model reads that result. It does not assign worker
tasks or edit project files. A successful live check depends on the provider's
current login, credits, startup, and tool-capable model.

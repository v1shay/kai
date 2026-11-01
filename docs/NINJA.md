# Ninja local inference

Ninja is a first-class Kai provider. Choose it in the existing provider/model picker in either the workspace or extended notch. Its additional controls select the inference machine, Ollama or llama.cpp, and the local Codex or Claude Code harness. The selected machine supplies model responses; project files, tools, MCP servers, approvals, Git operations, and editing remain on the Mac running Kai.

## Model picker

- Installed models come first, ordered by lab, followed by the public Ollama library. A bundled snapshot contains all 266 families listed on 2026-10-31. Opening Ninja refreshes the cached catalog in a worker, at most once per day.
- Search is local. Opening a family fetches its available tags. Clicking a tag downloads that model on the selected inference machine, with progress and a stop control.
- Tool, vision, context, and thinking capabilities come from Ollama `/api/show` or llama.cpp `/props`. Each engine's actual advertised thinking values are used. A model's reported default is selected when available. Models with no configurable thinking levels get no intensity selector.
- Coding harnesses require models that advertise tool calling. A plain completion/embedding model cannot acquire tool support by being placed behind a harness. Cloud-only Ollama tags are not local inference targets.
- Common lab marks use bundled SVGs. Sources and licenses are recorded in `integrations/Code/assets/NINJA-LOGOS.md`. Unknown publishers retain their own name rather than an invented brand mark.

Selections are saved per chat; explicit selections also become the default for future Ninja chats. Changing a running chat's selection applies to its next turn. Switching between Codex and Claude transfers the existing chat context through Kai's existing handoff mechanism. Native session IDs are never passed across different harnesses.

## Inference host

The optional `kai_ninja_host.py` helper manages inventory, downloads, and engine startup. It has no workspace, shell-command, arbitrary-file, or coding-tool endpoint.

On the inference Mac, install Ollama and/or llama.cpp and connect Tailscale. Then run the helper's explicit setup command, using the login name shown by your Tailscale account:

```sh
python3 kai_ninja_host.py --owner you@example.com --install
```

The macOS installer creates a user LaunchAgent named `com.kai.ninja.inference`. By default it binds only the machine's Tailscale IPv4 address on port 32187. Every request is authorized against Tailscale's `whois` identity and the configured owner. Incoming identity headers are ignored in this mode. Traffic stays inside Tailscale's encrypted network; no separate application password or public tunnel is used.

An explicit `--serve` installation instead binds loopback behind Tailscale Serve's `/ninja` HTTPS path and verifies Serve's authenticated user header. It preserves unrelated Serve paths and never enables Funnel. Foreground hosting on Linux is also possible with `--bind <tailscale-ip> --identity whois --owner <login>`; the automatic service installer currently targets macOS.

Already-running, directly reachable Ollama or llama.cpp servers can be selected without this helper. Cross-engine model downloads and automatic llama.cpp startup require the helper. llama.cpp can reuse an installed Ollama GGUF model layer, so the weights are downloaded once. Non-GGUF models cannot be loaded by llama.cpp.

Only helper-owned engine processes are stopped. Ollama controls its own idle model retention. The helper serializes its single llama.cpp server, queues model/inference operations, and releases that server after five idle minutes. It leaves independently started inference servers alone.

## Harness bridge and isolation

One private loopback bridge is shared by active Ninja harnesses. It translates Codex Responses and Claude Messages requests to the chosen inference API, including visible reasoning, native function calls, Codex freeform patch calls, tool results, and images. Tool execution remains inside the harness. Native subagent and approval events use the same Kai ownership/status/pet infrastructure as other providers.

Generation defaults to deterministic sampling when the harness omits a temperature; explicit harness sampling values are preserved on both engines. The bridge also decodes Qwen3-Coder's declared XML tool dialect when an engine misses its opening wrapper, the behavior documented in [Ollama issue #16686](https://github.com/ollama/ollama/issues/16686) and [llama.cpp's native parser](https://github.com/ggml-org/llama.cpp/blob/master/common/parsers/qwen3-coder.cpp). This decoder is restricted to that model family, complete native blocks, declared tool names, and typed parameters. Markdown examples, unknown names, partial blocks, and responses that already contain structured calls remain text. Decoded calls pass through the same native harness validation and approvals.

Codex receives explicit local-model metadata so patch and subagent tools are available, with the selected model's real context and image limits. Local skills, rules, configured roles, plugins, and MCP configuration remain available. Claude inherits its configured skills, agents, commands, plugins, and MCP servers. Ninja's per-route configuration profiles are separate from normal Codex/Claude settings and share only Ninja's own native history. Chat titles also use the selected local model.

The bridge uses an ephemeral local credential, private route/config files, and no inherited cloud authorization upstream. Inference endpoints must be local, LAN, or tailnet addresses. Private requests bypass inherited HTTP proxies, and inference redirects are refused. No cloud model is substituted on failure. The bridge exits when its owning Kai process exits, including an unexpected exit.

OpenAI-hosted server tools are not local harness tools. Codex's cloud-hosted web search is disabled for Ninja; configured MCP/local search tools remain available. The local model still determines tool-following quality, and capabilities vary between models and engines.

## Verification

```sh
/usr/bin/python3 scripts/test-ninja.py
cargo test --offline --manifest-path integrations/Code/Cargo.toml --bin orbit ninja
python3 scripts/test-ninja-native.py --binary <orbit-test-binary> \
  --endpoint http://<tailscale-ip>:32187 --model <installed-model:tag> --node --matrix
```

The explicit live test uses temporary projects and isolated native homes. It reads a local proof file, deletes it, then resumes the native chat through a different context/profile configuration and verifies the remembered result. It exercises both harnesses with both engines. No existing user chat is used.

Build 0.12.24/61 passed 18 protocol/authorization checks, four Ninja state/routing tests, and all four live engine/harness combinations with `qwen3-coder:30b` on the Studio. Signed-app startup checks passed for workspace, extended, and compact surfaces, including Ninja-selected workspace/extended fixtures. Chat-switching and idle probes also completed. The inference host remained responsive during a model reload. Temporary SSH setup access was removed after verification. Visual review remains a manual check; these results do not certify every registry model's tool-following quality.

The broad Rust suite had seven failures on the unchanged starting revision `fe4929b`; the same seven failed with Ninja. They concern existing pin ordering, removed toolbar selectors, navigation/draft fixtures, and a diff fixture timeout. The full run passed 903 tests with 22 ignored. Ninja-specific checks and live matrix results are recorded separately under `.build/validation/ninja/`.

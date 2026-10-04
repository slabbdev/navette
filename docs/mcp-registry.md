# Publishing navette to the official MCP Registry

The manifest lives at the repo root (`server.json`) and follows the
[official server.json schema](https://github.com/modelcontextprotocol/registry/blob/main/docs/reference/server-json/draft/server.schema.json).

## Publish (maintainers)

```sh
# one-time: install the publisher CLI
brew install mcp-publisher      # or: curl the release from
# https://github.com/modelcontextprotocol/registry/releases

mcp-publisher login github      # opens the browser, binds io.github.slabbdev
mcp-publisher publish           # reads ./server.json, pushes to
                                # https://registry.modelcontextprotocol.io
```

The name `io.github.slabbdev/navette` is auto-verified once you publish
while authenticated as the GitHub user `slabbdev`.

Bump `version` in `server.json` on every release (same value as Cargo.toml).

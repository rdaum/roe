# Mica in Roe

Roe's production editor policy is split between two Mica source units:

- `roe-model.mica` defines the editor ontology, derived relations, authority rules, and generic
  behaviors.
- `roe-first-wave.mica` defines the policy shipped with Roe: commands, key bindings, modes, faces,
  syntax rules, prompts, and configuration.

`roe-core` embeds both files and loads them for every production `WorkspaceHost`. The Rust host
publishes workspace-local sessions, buffers, views, and native-resource associations as volatile
facts. Native resource identifiers and capabilities never belong in these source units.

`MICA-REVISION` records the exact Mica revision pinned in the workspace `Cargo.toml`. A revision
change is an integration change, not an incidental dependency update.

Use the real Roe embedding when checking changes:

```sh
cargo test -p roe-core mica_ -- --test-threads=1
./scripts/check.sh
```

Change `roe-model.mica` when the relational model or generic behavior changes. Change
`roe-first-wave.mica` for shipped commands, bindings, modes, faces, and other editor policy. Keep
meaning in Mica and bounded native mechanisms in Rust.

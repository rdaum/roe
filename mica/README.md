# Roe Mica units

`roe-model.mica` is the checked Phase 3 ontology and durable `roe/core` prototype. It separates
durable programmable description from session-volatile logical state and host-owned native
associations. `roe-model-demo.mica` supplies non-installed fixture tuples and executes one `C-x o`
keymap-to-command-to-window workflow entirely in Mica.

`MICA-REVISION` is the exact Mica revision against which this source and the Phase 3 embedding ADRs
were checked. Phase 4 must use the same revision for the CPU-only `mica-driver` dependency with
default features disabled; changing the revision requires rechecking the unit and the driver API
decisions.

From a sibling checkout of Mica at that revision, validate the unit through the same driver-backed
runner path Roe will embed with:

```sh
cargo run --manifest-path ../mica/Cargo.toml -p mica-runner --bin mica -- \
  eval --filein "$PWD/mica/roe-model.mica" --filein "$PWD/mica/roe-model-demo.mica" \
  --actor roe/demo_actor \
  'return roe/dispatch_key(#roe/demo_actor, #roe/demo_session, "C-x o")'
```

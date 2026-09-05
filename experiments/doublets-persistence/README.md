# doublets-persistence

Evidence for the one design decision in `src/associative/native.rs`: a
file-mapped `doublets` 0.5 store does **not** survive being reopened, so the
durable projection of the link network has to be the `.lino` document rather
than the mapping.

Run it with:

```bash
cargo run --manifest-path experiments/doublets-persistence/Cargo.toml
```

Output on `doublets` 0.5.0 with `platform-mem` 0.3.0:

```text
session1 wrote 1 2 3 count=3
after session1 @item 0: [0, 0, 0, 0, 0, 0, 0, 0]
after session1 @item 8192: [3, 1048575, 0, 0, 3, 3, 0, 0]
session2 count=0 link3=None
inside session2 @item 8192: [0, 1048575, 0, 0, 0, 0, 0, 0]
```

The first session's links do reach the file — the store header
(`allocated = 3`, both tree roots `= 3`) is written at item 8192, because
`unit::Store::init` grows the mapping twice and keeps only the second, added
region. Reopening the same file wipes it: `doublets::mem::resize_mem` grows
with [`RawMem::grow_filled`], which fills the whole newly mapped region with
`LinkPart::default()` regardless of how much of it the backend reported as
already initialised. (`RawMem::grow_filled_exact` is the variant that honours
that count, but `resize_mem` does not call it.) The header at item 8192 is
therefore reset to zero and the links become unreachable.

Consequences for this crate:

- `NativeStore::in_memory` and `NativeStore::file_mapped` are both useful
  *within* a run — the file-mapped backend lets a network that does not fit in
  RAM spill to disk;
- nothing in the crate may treat the mapping as an archive; `.lino` is the
  archive.

[`RawMem::grow_filled`]: https://docs.rs/platform-mem/0.3.0/platform_mem/raw_mem/trait.RawMem.html#method.grow_filled

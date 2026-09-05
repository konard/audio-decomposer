# lino-probe

A throwaway program that answers one question with executable evidence: which
Links Notation shapes survive a `parse` → `format` → `parse` round trip in
`links-notation` 0.17?

Run it with:

```bash
cargo run --manifest-path experiments/lino-probe/Cargo.toml
```

Findings that the crate's `associative::lino` module relies on:

- `(id: value ...)` links and nested anonymous links round trip unchanged;
- quoted strings containing spaces do **not** round trip, because the formatter
  drops the quotes, so every value this crate writes must be a whitespace-free
  token;
- identifiers must likewise be single tokens.

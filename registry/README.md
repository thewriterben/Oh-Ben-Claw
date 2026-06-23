# Hardware Registry — Single Source of Truth

The canonical Oh-Ben-Claw hardware catalog lives in Rust at
**`src/peripherals/registry.rs`** (`KNOWN_BOARDS`, `KNOWN_ACCESSORIES`,
`Connector`). Everything else — the OBC deployment generator, Accelerapp — must
**consume a generated export**, never re-type the catalog.

## Generate `registry.json`

```bash
# Pass the output path so the binary writes UTF-8 itself.
# (Do NOT use `> registry.json` in PowerShell — it writes UTF-16 + BOM and breaks consumers.)
cargo run --bin emit-registry -- registry/registry.json
```

This serializes the live registry to a stable JSON document:

```jsonc
{
  "schema_version": 1,
  "boards":      [ { "vid", "pid", "name", "architecture", "transport",
                     "capabilities", "vendor", "ecosystem", "connectors" }, … ],
  "accessories": [ { "name", "description", "bus", "default_i2c_addr",
                     "capabilities", "compatible_boards", "connector" }, … ]
}
```

- `vid`/`pid` are decimal `u16`; `default_i2c_addr` is a decimal `u8` or `null`.
- `connector(s)` serialize to stable lowercase tokens (`grove`, `qwiic`,
  `stemma_qt`, `mbus`, `featherwing`, `pmod`, `hat_pi`, `bare`).
- Bump `REGISTRY_SCHEMA_VERSION` in `registry.rs` on any breaking shape change.

## Consumers (Ecosystem Integration · I1)

- **OBC deployment generator** (`lib/obc-data.ts`): replace the hand-written
  `KNOWN_BOARDS` with an import of `registry.json`. Add a CI check that fails if
  a hand-written board list reappears.
- **Accelerapp**: align its platform/board list to the same `registry.json`.
- **Weekly hardware scout**: proposes edits to `registry.rs` → regenerate
  `registry.json` → all consumers update with zero re-typing.

> The committed `registry.json` is a build artifact: regenerate it via the
> command above whenever `registry.rs` changes (wire it into CI so it can't drift).

## CI drift guard

Wire these so the catalog can never silently drift across repos:

**Oh-Ben-Claw** — fail CI if `registry.json` is stale vs the Rust source of truth:

```bash
cargo run --bin emit-registry -- registry/registry.json
git diff --exit-code registry/registry.json   # non-zero exit ⇒ someone edited registry.rs without regenerating
```

**OBC deployment generator** — `lib/registry.json` is the bundled copy; its
`tests/obc-data.test.ts` "registry single source of truth" suite fails if the
data stops coming from `registry.json` (e.g. a hand-written board list is
re-introduced) or if counts/schema drift. On any registry change, refresh the
bundled copy and run the suite:

```bash
cp ../Oh-Ben-Claw/registry/registry.json lib/registry.json
npm test
```

> Encoding note: always generate with the `-- <path>` argument (the binary
> writes UTF-8). Do **not** use `> registry.json` in PowerShell — it writes
> UTF-16 + BOM and breaks JSON consumers.

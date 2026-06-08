//! Generates the committed JSON Schema artifact.
//!
//! Run with: `cargo run -p fleet-protocol --bin gen-schema`
//! Writes `crates/fleet-protocol/schema/fleet-protocol.schema.json` relative to
//! the crate root. The conformance test asserts the on-disk file matches this
//! output, so CI fails if someone changes a type without regenerating.

#[cfg(feature = "schema")]
fn main() -> std::io::Result<()> {
    let out = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/schema/fleet-protocol.schema.json"
    );
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/schema");
    std::fs::create_dir_all(dir)?;
    std::fs::write(out, fleet_protocol::schema::combined_schema_json())?;
    eprintln!("wrote {out}");
    Ok(())
}

#[cfg(not(feature = "schema"))]
fn main() {
    eprintln!("gen-schema requires the `schema` feature; run with --features schema");
    std::process::exit(1);
}
